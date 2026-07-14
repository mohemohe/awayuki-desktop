//! Bluesky client wrapping `bsky-sdk`'s `BskyAgent`.
//!
//! Bluesky uses the AT Protocol. Authentication is via username (handle / DID / email)
//! and an app password, exchanged for an access JWT and a refresh JWT through
//! `com.atproto.server.createSession`. The agent transparently refreshes tokens.
//!
//! For persistence we serialise the entire `AtpSession` (which carries access/refresh JWTs,
//! DID, handle, etc.) as JSON and persist it in Awayuki's SQLite database.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use atrium_api::agent::atp_agent::store::MemorySessionStore;
use atrium_api::agent::atp_agent::AtpSession;
use atrium_api::agent::Configure;
use atrium_api::did_doc::DidDocument;
use atrium_api::types::TryFromUnknown;
use bsky_sdk::BskyAgent;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::api::http::{
    api_client, body_bytes_limited, body_text_limited, MAX_API_RESPONSE_BYTES,
    MAX_ERROR_RESPONSE_BYTES,
};
use crate::bluesky::rate_limit::{self, RateLimitState};
use crate::bluesky::xrpc::RateLimitTrackingClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::Status;

const NOTIFICATION_SUBJECT_CACHE_CAPACITY: usize = 512;
const NOTIFICATION_SUBJECT_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const NOTIFICATION_SUBJECT_NEGATIVE_TTL: Duration = Duration::from_secs(30);

struct CachedValue<T> {
    value: T,
    fetched_at: Instant,
    last_accessed: Instant,
}

struct BoundedTtlCache<T> {
    entries: HashMap<String, CachedValue<T>>,
    capacity: usize,
    ttl: Duration,
}

impl<T: Clone> BoundedTtlCache<T> {
    fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            ttl,
        }
    }

    fn get_many(&mut self, keys: &[String], now: Instant) -> HashMap<String, T> {
        self.entries
            .retain(|_, entry| now.duration_since(entry.fetched_at) <= self.ttl);
        let mut found = HashMap::new();
        for key in keys {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.last_accessed = now;
                found.insert(key.clone(), entry.value.clone());
            }
        }
        found
    }

    fn insert(&mut self, key: String, value: T, now: Instant) {
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            key,
            CachedValue {
                value,
                fetched_at: now,
                last_accessed: now,
            },
        );
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

/// `BskyAgent` parameterised with our rate-limit tracking client. All
/// endpoint calls go through `agent.api.…`, which uses the trait-bounded
/// type internally and exposes the same surface regardless of which
/// `XrpcClient` impl is plugged in — so the rest of the Bluesky module
/// doesn't have to know we swapped the default `ReqwestClient`.
pub type TrackedBskyAgent = BskyAgent<RateLimitTrackingClient, MemorySessionStore>;

/// The default Bluesky entry-point. Self-hosted PDSes are resolved via the user's DID
/// document after `createSession` succeeds, so we always start at bsky.social.
pub const DEFAULT_BLUESKY_HOST: &str = "bsky.social";

/// Application-owned destination for a rotated Bluesky session. Refresh is
/// not reported as successful until this future completes, preventing a new
/// refresh-token family from existing only in process memory.
pub trait BlueskyCredentialSink: Send + Sync {
    fn persist(
        &self,
        access_token: String,
        app_password: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), MastodonError>> + Send + '_>>;
}

/// Wrapper around `BskyAgent`. Implements `Clone` by sharing the underlying
/// agent through `Arc`.
#[derive(Clone)]
pub struct BlueskyClient {
    agent: Arc<TrackedBskyAgent>,
    domain: String,
    /// Stored as JSON-serialised `AtpSession`. Refreshed automatically by the agent;
    /// the workspace persists this value to SQLite.
    access_token: Arc<StdRwLock<String>>,
    pub streaming_url: String,
    /// Latest `RateLimit-*` snapshot from any response on this account's
    /// agent. Updated in-band by the wrapping XRPC client; the UI polls it
    /// for display in Settings → Account.
    rate_limit_state: RateLimitState,
    /// App password used at original login time. Held so the workspace can
    /// re-create the session via `com.atproto.server.createSession` whenever
    /// the stored access/refresh JWTs are rejected (Bluesky periodically
    /// invalidates JWTs — e.g. after sleep, after handle changes, or when
    /// the refresh JWT family is rotated server-side).
    app_password: Arc<StdRwLock<Option<String>>>,
    auth_recovery: AuthRecoveryGate,
    credential_sink: Arc<StdRwLock<Option<Arc<dyn BlueskyCredentialSink>>>>,
    credential_persist: Arc<Mutex<()>>,
    notification_subject_cache: Arc<Mutex<BoundedTtlCache<Status>>>,
    notification_subject_misses: Arc<Mutex<BoundedTtlCache<()>>>,
}

/// Persisted form of a Bluesky session — what we put into `access_token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub session: AtpSession,
}

#[derive(Debug, Clone)]
enum SharedAuthRecoveryOutcome {
    Success,
    Unauthorized,
    Failed(String),
}

impl SharedAuthRecoveryOutcome {
    fn capture(result: &Result<(), MastodonError>) -> Self {
        match result {
            Ok(()) => Self::Success,
            Err(MastodonError::Unauthorized) => Self::Unauthorized,
            Err(error) => Self::Failed(error.to_string()),
        }
    }

    fn into_result(self) -> Result<(), MastodonError> {
        match self {
            Self::Success => Ok(()),
            Self::Unauthorized => Err(MastodonError::Unauthorized),
            Self::Failed(message) => Err(MastodonError::Other(message)),
        }
    }
}

#[derive(Debug, Default)]
struct AuthRecoveryState {
    last_generation: Option<u64>,
    last_outcome: Option<SharedAuthRecoveryOutcome>,
}

/// Serializes a rotating refresh-token family and lets every request that
/// observed the same generation share the first request's result. Advancing
/// the generation on failures is intentional: queued 401 handlers must not
/// each replay a failed rotation, while a later independent 401 can retry.
#[derive(Clone, Debug, Default)]
struct AuthRecoveryGate {
    generation: Arc<AtomicU64>,
    state: Arc<Mutex<AuthRecoveryState>>,
    #[cfg(test)]
    entrants: Arc<AtomicU64>,
}

impl AuthRecoveryGate {
    async fn run<F, Fut>(&self, recover: F) -> Result<(), MastodonError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), MastodonError>>,
    {
        let observed_generation = self.generation.load(Ordering::Acquire);
        #[cfg(test)]
        self.entrants.fetch_add(1, Ordering::AcqRel);
        let mut state = self.state.lock().await;
        let current_generation = self.generation.load(Ordering::Acquire);
        if current_generation != observed_generation {
            return match (state.last_generation, state.last_outcome.as_ref().cloned()) {
                (Some(generation), Some(outcome)) if generation == current_generation => {
                    outcome.into_result()
                }
                // A generation invalidated by logout has no recovery result to
                // share. Treat it as logged out instead of running stale work.
                _ => Err(MastodonError::Unauthorized),
            };
        }

        let result = recover().await;
        let outcome = SharedAuthRecoveryOutcome::capture(&result);
        let next_generation = observed_generation.wrapping_add(1);
        if self
            .generation
            .compare_exchange(
                observed_generation,
                next_generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(MastodonError::Unauthorized);
        }
        state.last_generation = Some(next_generation);
        state.last_outcome = Some(outcome);
        result
    }

    async fn invalidate_for_logout(&self) {
        // Recovery holds this mutex through token rotation and SQLite sink
        // persistence. Logout waits for that work, advances the generation,
        // and then deletes the account row, so a stale refresh can never
        // recreate credentials after logout has completed.
        let mut state = self.state.lock().await;
        self.generation.fetch_add(1, Ordering::AcqRel);
        state.last_generation = None;
        state.last_outcome = None;
    }

    #[cfg(test)]
    fn entrant_count(&self) -> u64 {
        self.entrants.load(Ordering::Acquire)
    }
}

impl BlueskyClient {
    /// Build a client from an existing session payload (the JSON we stored).
    ///
    /// `resume_session` itself calls `com.atproto.server.getSession`, and atrium-api's
    /// auto-refresh middleware only fires on `ExpiredToken` — `AuthMissing` /
    /// `InvalidToken` go through unhandled. When the access JWT has been invalidated
    /// for any other reason (or the snapshot-on-disk lagged behind in-memory rotations),
    /// we fall back to a manual `com.atproto.server.refreshSession` call with the stored
    /// refresh JWT and retry resume with the freshly-issued tokens.
    pub async fn from_stored(
        domain: &str,
        access_token: String,
        streaming_url: String,
        app_password: Option<String>,
    ) -> Result<Self, MastodonError> {
        let stored: StoredSession = serde_json::from_str(&access_token)
            .map_err(|e| MastodonError::Other(format!("Bluesky session decode failed: {}", e)))?;

        let rate_limit_state = rate_limit::new_state();
        let xrpc = RateLimitTrackingClient::new(
            format!("https://{}", DEFAULT_BLUESKY_HOST),
            rate_limit_state.clone(),
        )?;
        let agent = BskyAgent::builder()
            .client(xrpc)
            .build()
            .await
            .map_err(|e| MastodonError::Other(format!("Bluesky agent build failed: {}", e)))?;

        // The default agent endpoint is bsky.social. If the stored session carries a
        // did_doc that points elsewhere (self-hosted PDS), point the agent at it before
        // the first request so we don't talk to bsky.social with a token issued by a
        // different PDS.
        if let Some(endpoint) = stored
            .session
            .data
            .did_doc
            .as_ref()
            .and_then(|value| DidDocument::try_from_unknown(value.clone()).ok())
            .and_then(|doc| doc.get_pds_endpoint())
        {
            agent.configure_endpoint(endpoint);
        }

        let resume_endpoint = endpoint_for_refresh(domain, &stored.session);

        if let Err(initial_err) = agent.resume_session(stored.session.clone()).await {
            tracing::warn!(
                "Bluesky resume_session failed ({}); attempting manual refresh",
                initial_err
            );
            let refreshed = match manual_refresh_session(&resume_endpoint, &stored.session).await {
                Ok(r) => r,
                Err(MastodonError::Unauthorized) => {
                    let password = app_password.clone().ok_or(MastodonError::Unauthorized)?;
                    let identifier = stored.session.data.handle.to_string();
                    tracing::warn!(
                        "Bluesky refreshSession failed during restore; re-authenticating via app password"
                    );
                    agent.login(&identifier, &password).await.map_err(|e| {
                        MastodonError::Other(format!(
                            "Bluesky app-password reauthentication failed: {}",
                            e
                        ))
                    })?;
                    let token = agent
                        .get_session()
                        .await
                        .ok_or_else(|| {
                            MastodonError::Other(
                                "Bluesky agent has no session after reauthentication".into(),
                            )
                        })
                        .and_then(|session| {
                            serde_json::to_string(&StoredSession { session }).map_err(|e| {
                                MastodonError::Other(format!(
                                    "Bluesky session encode failed: {}",
                                    e
                                ))
                            })
                        })?;
                    return Ok(Self {
                        agent: Arc::new(agent),
                        domain: domain.to_string(),
                        access_token: Arc::new(StdRwLock::new(token)),
                        streaming_url,
                        rate_limit_state,
                        app_password: Arc::new(StdRwLock::new(Some(password))),
                        auth_recovery: AuthRecoveryGate::default(),
                        credential_sink: Arc::new(StdRwLock::new(None)),
                        credential_persist: Arc::new(Mutex::new(())),
                        notification_subject_cache: Arc::new(Mutex::new(BoundedTtlCache::new(
                            NOTIFICATION_SUBJECT_CACHE_CAPACITY,
                            NOTIFICATION_SUBJECT_CACHE_TTL,
                        ))),
                        notification_subject_misses: Arc::new(Mutex::new(BoundedTtlCache::new(
                            NOTIFICATION_SUBJECT_CACHE_CAPACITY,
                            NOTIFICATION_SUBJECT_NEGATIVE_TTL,
                        ))),
                    });
                }
                Err(refresh_err) => {
                    return Err(MastodonError::Other(format!(
                        "Bluesky resume_session failed: {} (refresh fallback also failed: {})",
                        initial_err, refresh_err
                    )));
                }
            };
            agent.resume_session(refreshed).await.map_err(|e| {
                MastodonError::Other(format!(
                    "Bluesky resume_session failed after refresh: {} (initial: {})",
                    e, initial_err
                ))
            })?;
        }

        // Capture the post-resume session — it may have been refreshed and/or had
        // its did_doc / handle updated by `getSession`. Persisting this snapshot
        // back to the DB on the caller side keeps the next launch's stored token
        // in sync with what the agent actually used.
        let token = match agent.get_session().await {
            Some(session) => serde_json::to_string(&StoredSession { session }).map_err(|e| {
                MastodonError::Other(format!("Bluesky session encode failed: {}", e))
            })?,
            None => access_token,
        };

        Ok(Self {
            agent: Arc::new(agent),
            domain: domain.to_string(),
            access_token: Arc::new(StdRwLock::new(token)),
            streaming_url,
            rate_limit_state,
            app_password: Arc::new(StdRwLock::new(app_password)),
            auth_recovery: AuthRecoveryGate::default(),
            credential_sink: Arc::new(StdRwLock::new(None)),
            credential_persist: Arc::new(Mutex::new(())),
            notification_subject_cache: Arc::new(Mutex::new(BoundedTtlCache::new(
                NOTIFICATION_SUBJECT_CACHE_CAPACITY,
                NOTIFICATION_SUBJECT_CACHE_TTL,
            ))),
            notification_subject_misses: Arc::new(Mutex::new(BoundedTtlCache::new(
                NOTIFICATION_SUBJECT_CACHE_CAPACITY,
                NOTIFICATION_SUBJECT_NEGATIVE_TTL,
            ))),
        })
    }

    /// Build a client immediately after a fresh login, taking the active session.
    pub async fn from_agent(
        domain: &str,
        agent: TrackedBskyAgent,
        rate_limit_state: RateLimitState,
        streaming_url: String,
        app_password: Option<String>,
    ) -> Result<Self, MastodonError> {
        let session = agent
            .get_session()
            .await
            .ok_or_else(|| MastodonError::Other("Bluesky agent has no session".into()))?;

        let stored = StoredSession { session };
        let token = serde_json::to_string(&stored)
            .map_err(|e| MastodonError::Other(format!("Bluesky session encode failed: {}", e)))?;

        Ok(Self {
            agent: Arc::new(agent),
            domain: domain.to_string(),
            access_token: Arc::new(StdRwLock::new(token)),
            streaming_url,
            rate_limit_state,
            app_password: Arc::new(StdRwLock::new(app_password)),
            auth_recovery: AuthRecoveryGate::default(),
            credential_sink: Arc::new(StdRwLock::new(None)),
            credential_persist: Arc::new(Mutex::new(())),
            notification_subject_cache: Arc::new(Mutex::new(BoundedTtlCache::new(
                NOTIFICATION_SUBJECT_CACHE_CAPACITY,
                NOTIFICATION_SUBJECT_CACHE_TTL,
            ))),
            notification_subject_misses: Arc::new(Mutex::new(BoundedTtlCache::new(
                NOTIFICATION_SUBJECT_CACHE_CAPACITY,
                NOTIFICATION_SUBJECT_NEGATIVE_TTL,
            ))),
        })
    }

    pub(crate) async fn cached_notification_subjects(
        &self,
        uris: &[String],
    ) -> HashMap<String, Status> {
        self.notification_subject_cache
            .lock()
            .await
            .get_many(uris, Instant::now())
    }

    pub(crate) async fn cache_notification_subject(&self, uri: String, status: Status) {
        self.notification_subject_misses.lock().await.remove(&uri);
        self.notification_subject_cache
            .lock()
            .await
            .insert(uri, status, Instant::now());
    }

    pub(crate) async fn invalidate_notification_subject(&self, uri: &str) {
        self.notification_subject_cache.lock().await.remove(uri);
        self.notification_subject_misses.lock().await.remove(uri);
    }

    pub(crate) async fn cached_missing_notification_subjects(
        &self,
        uris: &[String],
    ) -> HashMap<String, ()> {
        self.notification_subject_misses
            .lock()
            .await
            .get_many(uris, Instant::now())
    }

    pub(crate) async fn cache_missing_notification_subject(&self, uri: String) {
        self.notification_subject_misses
            .lock()
            .await
            .insert(uri, (), Instant::now());
    }

    pub fn set_credential_sink(&self, sink: Arc<dyn BlueskyCredentialSink>) {
        *self
            .credential_sink
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    pub async fn invalidate_auth_generation(&self) {
        self.auth_recovery.invalidate_for_logout().await;
    }

    /// Snapshot of the persisted app password (if any). Used when persisting the
    /// account row so that the password we received at login (or carried from
    /// the previous DB row) survives token rotation writes.
    pub fn cached_app_password(&self) -> Option<String> {
        self.app_password
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Best-effort handle extraction from a stored `access_token` JSON blob.
    /// Used by the workspace to obtain the identifier for `createSession`
    /// when the stored session is rejected and we need to fall back to
    /// app-password re-login.
    pub fn extract_handle(access_token: &str) -> Option<String> {
        serde_json::from_str::<StoredSession>(access_token)
            .ok()
            .map(|stored| stored.session.data.handle.to_string())
    }

    pub fn agent(&self) -> &TrackedBskyAgent {
        &self.agent
    }

    /// Shared handle to the rate-limit slot. UI code that wants to poll the
    /// state at render time clones this once and reads it on each frame
    /// without going through `BlueskyClient`. `None` until the first
    /// rate-limited response — typically the initial `getSession`
    /// immediately after `from_stored`.
    pub fn rate_limit_state(&self) -> RateLimitState {
        self.rate_limit_state.clone()
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Returns the cached session-token snapshot (last value persisted from `refresh_token`).
    /// Use only when you cannot await — callers that need fresh post-refresh JWTs (e.g.
    /// before secure persistence) must use `refresh_token().await` instead.
    ///
    /// This lock only protects a short in-memory clone; it never falls back to an
    /// empty token, even if a previous holder panicked.
    pub fn cached_access_token(&self) -> String {
        self.access_token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Pull the agent's current session (which may have been silently refreshed by the
    /// agent's auto-refresh middleware), re-serialise it to JSON, update the snapshot,
    /// and return the up-to-date string. This is what should be persisted securely.
    pub async fn refresh_token(&self) -> Result<String, MastodonError> {
        let _persist_guard = self.credential_persist.lock().await;
        let session = self
            .agent
            .get_session()
            .await
            .ok_or_else(|| MastodonError::Other("Bluesky agent has no session".into()))?;
        let stored = StoredSession { session };
        let token = serde_json::to_string(&stored)
            .map_err(|e| MastodonError::Other(format!("Bluesky session encode failed: {}", e)))?;

        if self
            .access_token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_str()
            == token
        {
            return Ok(token);
        }

        let sink = self
            .credential_sink
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(sink) = sink {
            let app_password = self.cached_app_password();
            sink.persist(token.clone(), app_password).await?;
        }
        *self
            .access_token
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = token.clone();
        Ok(token)
    }

    /// Recover an authenticated agent after an endpoint returns a 401-style XRPC
    /// error. Prefer refreshSession so normal token rotation remains cheap; if the
    /// refresh JWT is revoked/expired, recreate the session from the saved app password.
    pub async fn recover_authentication(&self) -> Result<(), MastodonError> {
        self.auth_recovery
            .run(|| async {
                crate::observability::observe_http_retry();

                if let Some(session) = self.agent.get_session().await {
                    let endpoint = endpoint_for_refresh(&self.domain, &session);
                    match manual_refresh_session(&endpoint, &session).await {
                        Ok(refreshed) => {
                            tracing::warn!(
                                "Bluesky endpoint returned unauthorized; refreshed session"
                            );
                            self.agent.resume_session(refreshed).await.map_err(|e| {
                                MastodonError::Other(format!(
                                    "Bluesky resume_session failed after endpoint refresh: {}",
                                    e
                                ))
                            })?;
                            self.refresh_token().await?;
                            return Ok(());
                        }
                        Err(MastodonError::Unauthorized) => {
                            tracing::warn!(
                                "Bluesky refreshSession failed after endpoint 401; falling back to app password"
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }

                self.reauthenticate_with_app_password().await
            })
            .await
    }

    pub fn is_auth_error(error: &MastodonError) -> bool {
        match error {
            MastodonError::Unauthorized => true,
            MastodonError::Other(message) => is_bluesky_auth_error_message(message),
            _ => false,
        }
    }

    async fn reauthenticate_with_app_password(&self) -> Result<(), MastodonError> {
        let password = self
            .app_password
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(MastodonError::Unauthorized)?;
        let identifier = self
            .agent
            .get_session()
            .await
            .map(|session| session.data.handle.to_string())
            .or_else(|| self.cached_stored_handle())
            .ok_or(MastodonError::Unauthorized)?;

        self.agent
            .login(&identifier, &password)
            .await
            .map_err(|e| {
                MastodonError::Other(format!(
                    "Bluesky app-password reauthentication failed: {}",
                    e
                ))
            })?;
        self.refresh_token().await?;
        Ok(())
    }

    fn cached_stored_handle(&self) -> Option<String> {
        let token = self
            .access_token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::extract_handle(&token)
    }
}

fn is_bluesky_auth_error_message(message: &str) -> bool {
    (message.contains("401")
        || message.contains("AuthMissing")
        || message.contains("ExpiredToken")
        || message.contains("InvalidToken"))
        && (message.contains("xrpc") || message.contains("Authentication Required"))
}

/// Resolve the URL we should hit to refresh the session. Prefers the PDS endpoint
/// recorded in the stored session's did_doc (matters for self-hosted PDSes), and
/// falls back to the account's `domain` so we always have a host to talk to.
fn endpoint_for_refresh(domain: &str, session: &AtpSession) -> String {
    session
        .data
        .did_doc
        .as_ref()
        .and_then(|value| DidDocument::try_from_unknown(value.clone()).ok())
        .and_then(|doc| doc.get_pds_endpoint())
        .unwrap_or_else(|| format!("https://{}", domain))
}

/// Call `com.atproto.server.refreshSession` directly with the stored refresh JWT,
/// returning a session struct with the freshly-issued access/refresh tokens copied
/// onto the existing fields (DID, handle, did_doc carry over).
///
/// We bypass `BskyAgent` here because the agent's auto-refresh path inside
/// `resume_session` only fires on `ExpiredToken`; for `AuthMissing` /
/// `InvalidToken` (which Bluesky returns when an old access JWT can't be decoded
/// or has been server-side invalidated) the refresh never runs and resume fails
/// permanently.
async fn manual_refresh_session(
    endpoint: &str,
    session: &AtpSession,
) -> Result<AtpSession, MastodonError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RefreshOutput {
        access_jwt: String,
        refresh_jwt: String,
    }

    let url = format!(
        "{}/xrpc/com.atproto.server.refreshSession",
        endpoint.trim_end_matches('/')
    );
    let resp = api_client()?
        .post(&url)
        .bearer_auth(&session.data.refresh_jwt)
        .send()
        .await
        .map_err(|e| MastodonError::Other(format!("refreshSession request failed: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        let body = body_text_limited(resp, MAX_ERROR_RESPONSE_BYTES)
            .await
            .unwrap_or_default();
        // 4xx from refreshSession means the refresh JWT itself is dead
        // (revoked, expired, malformed, account taken down, …). The client cannot
        // recover without re-authentication, so surface `Unauthorized` to let the
        // caller drop the stored session and prompt for fresh login.
        if status.is_client_error() {
            tracing::warn!(
                "refreshSession returned {} (refresh JWT permanently invalid): {}",
                status,
                body
            );
            return Err(MastodonError::Unauthorized);
        }
        return Err(MastodonError::Other(format!(
            "refreshSession returned {}: {}",
            status, body
        )));
    }

    let body = body_bytes_limited(resp, MAX_API_RESPONSE_BYTES)
        .await
        .map_err(|e| MastodonError::Other(format!("refreshSession body failed: {}", e)))?;
    let out: RefreshOutput = serde_json::from_slice(&body)
        .map_err(|e| MastodonError::Other(format!("refreshSession decode failed: {}", e)))?;

    let mut refreshed = session.clone();
    refreshed.data.access_jwt = out.access_jwt;
    refreshed.data.refresh_jwt = out.refresh_jwt;
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tokio::sync::Notify;

    #[test]
    fn notification_subject_cache_is_bounded_and_expires_entries() {
        let now = Instant::now();
        let mut cache = BoundedTtlCache::new(2, Duration::from_secs(60));
        cache.insert("one".to_string(), 1, now);
        cache.insert("two".to_string(), 2, now + Duration::from_secs(1));
        assert_eq!(
            cache.get_many(&["one".to_string()], now + Duration::from_secs(2)),
            HashMap::from([("one".to_string(), 1)])
        );
        cache.insert("three".to_string(), 3, now + Duration::from_secs(3));

        assert!(cache
            .get_many(&["two".to_string()], now + Duration::from_secs(4))
            .is_empty());
        assert_eq!(cache.entries.len(), 2);
        assert!(cache
            .get_many(&["one".to_string()], now + Duration::from_secs(63))
            .is_empty());
    }

    #[tokio::test]
    async fn simultaneous_unauthorized_requests_share_one_refresh_result() {
        let gate = AuthRecoveryGate::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());

        let first = {
            let gate = gate.clone();
            let calls = calls.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                gate.run(|| async move {
                    calls.fetch_add(1, AtomicOrdering::AcqRel);
                    started.notify_one();
                    release.notified().await;
                    Ok(())
                })
                .await
            })
        };
        started.notified().await;

        let mut waiters = Vec::new();
        for _ in 0..7 {
            let gate = gate.clone();
            let calls = calls.clone();
            waiters.push(tokio::spawn(async move {
                gate.run(|| async move {
                    calls.fetch_add(1, AtomicOrdering::AcqRel);
                    Ok(())
                })
                .await
            }));
        }
        while gate.entrant_count() < 8 {
            tokio::task::yield_now().await;
        }
        release.notify_one();

        assert!(first.await.expect("first task").is_ok());
        for waiter in waiters {
            assert!(waiter.await.expect("waiter task").is_ok());
        }
        assert_eq!(calls.load(AtomicOrdering::Acquire), 1);
    }

    #[tokio::test]
    async fn token_rotation_failure_is_shared_but_a_later_request_can_retry() {
        let gate = AuthRecoveryGate::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());

        let first = {
            let gate = gate.clone();
            let calls = calls.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                gate.run(|| async move {
                    calls.fetch_add(1, AtomicOrdering::AcqRel);
                    started.notify_one();
                    release.notified().await;
                    Err(MastodonError::Other("rotation persistence failed".into()))
                })
                .await
            })
        };
        started.notified().await;
        let waiter = {
            let gate = gate.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                gate.run(|| async move {
                    calls.fetch_add(1, AtomicOrdering::AcqRel);
                    Ok(())
                })
                .await
            })
        };
        while gate.entrant_count() < 2 {
            tokio::task::yield_now().await;
        }
        release.notify_one();

        let first_error = first
            .await
            .expect("first task")
            .expect_err("rotation must fail")
            .to_string();
        let waiter_error = waiter
            .await
            .expect("waiter task")
            .expect_err("waiter must share failure")
            .to_string();
        assert!(first_error.contains("rotation persistence failed"));
        assert_eq!(waiter_error, first_error);
        assert_eq!(calls.load(AtomicOrdering::Acquire), 1);

        gate.run(|| async {
            calls.fetch_add(1, AtomicOrdering::AcqRel);
            Ok(())
        })
        .await
        .expect("later retry");
        assert_eq!(calls.load(AtomicOrdering::Acquire), 2);
    }

    #[tokio::test]
    async fn logout_waits_for_in_flight_rotation_before_advancing_generation() {
        let gate = AuthRecoveryGate::default();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let recovery = {
            let gate = gate.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                gate.run(|| async move {
                    started.notify_one();
                    release.notified().await;
                    Ok(())
                })
                .await
            })
        };
        started.notified().await;
        let invalidation = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.invalidate_for_logout().await })
        };
        tokio::task::yield_now().await;
        assert!(!invalidation.is_finished());
        release.notify_one();

        assert!(recovery.await.expect("recovery task").is_ok());
        invalidation.await.expect("logout invalidation");
        assert_eq!(gate.generation.load(Ordering::Acquire), 2);
    }
}
