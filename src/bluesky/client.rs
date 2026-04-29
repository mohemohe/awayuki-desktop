//! Bluesky client wrapping `bsky-sdk`'s `BskyAgent`.
//!
//! Bluesky uses the AT Protocol. Authentication is via username (handle / DID / email)
//! and an app password, exchanged for an access JWT and a refresh JWT through
//! `com.atproto.server.createSession`. The agent transparently refreshes tokens.
//!
//! For persistence we serialise the entire `AtpSession` (which carries access/refresh JWTs,
//! DID, handle, etc.) as JSON and store it in `login_accounts.access_token`.

use std::sync::Arc;

use atrium_api::agent::Configure;
use atrium_api::agent::atp_agent::AtpSession;
use atrium_api::did_doc::DidDocument;
use atrium_api::types::TryFromUnknown;
use bsky_sdk::BskyAgent;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::mastodon::error::MastodonError;

/// The default Bluesky entry-point. Self-hosted PDSes are resolved via the user's DID
/// document after `createSession` succeeds, so we always start at bsky.social.
pub const DEFAULT_BLUESKY_HOST: &str = "bsky.social";

/// Wrapper around `BskyAgent`. Implements `Clone` by sharing the underlying
/// agent through `Arc`.
#[derive(Clone)]
pub struct BlueskyClient {
    agent: Arc<BskyAgent>,
    domain: String,
    /// Stored as JSON-serialised `AtpSession`. Refreshed automatically by the agent;
    /// the workspace persists this value back to the DB on shutdown via `current_session_json`.
    access_token: Arc<RwLock<String>>,
    pub streaming_url: String,
}

/// Persisted form of a Bluesky session — what we put into `access_token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub session: AtpSession,
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
    ) -> Result<Self, MastodonError> {
        let stored: StoredSession = serde_json::from_str(&access_token)
            .map_err(|e| MastodonError::Other(format!("Bluesky session decode failed: {}", e)))?;

        let agent = BskyAgent::builder()
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
                // Permanent auth failure — the refresh JWT was revoked or expired
                // server-side. Bubble up `Unauthorized` so the workspace can drop
                // this account from the DB and route the user to the login screen.
                Err(MastodonError::Unauthorized) => return Err(MastodonError::Unauthorized),
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
            access_token: Arc::new(RwLock::new(token)),
            streaming_url,
        })
    }

    /// Build a client immediately after a fresh login, taking the active session.
    pub async fn from_agent(
        domain: &str,
        agent: BskyAgent,
        streaming_url: String,
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
            access_token: Arc::new(RwLock::new(token)),
            streaming_url,
        })
    }

    pub fn agent(&self) -> &BskyAgent {
        &self.agent
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Returns the cached session-token snapshot (last value persisted from `refresh_token`).
    /// Use only when you cannot await — callers that need fresh post-refresh JWTs (e.g.
    /// before saving to DB) must use `refresh_token().await` instead.
    ///
    /// Falls back to the prior snapshot if the lock is contended, so we never hand out
    /// an empty string that would clobber the stored token on next save.
    pub fn cached_access_token(&self) -> String {
        self.access_token
            .try_read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| {
                tracing::warn!("Bluesky access_token snapshot lock contended; reusing last known");
                String::new()
            })
    }

    /// Pull the agent's current session (which may have been silently refreshed by the
    /// agent's auto-refresh middleware), re-serialise it to JSON, update the snapshot,
    /// and return the up-to-date string. This is what should be persisted to the DB.
    pub async fn refresh_token(&self) -> Result<String, MastodonError> {
        let session = self
            .agent
            .get_session()
            .await
            .ok_or_else(|| MastodonError::Other("Bluesky agent has no session".into()))?;
        let stored = StoredSession { session };
        let token = serde_json::to_string(&stored)
            .map_err(|e| MastodonError::Other(format!("Bluesky session encode failed: {}", e)))?;
        *self.access_token.write().await = token.clone();
        Ok(token)
    }
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
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&session.data.refresh_jwt)
        .send()
        .await
        .map_err(|e| MastodonError::Other(format!("refreshSession request failed: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
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

    let out: RefreshOutput = resp
        .json()
        .await
        .map_err(|e| MastodonError::Other(format!("refreshSession decode failed: {}", e)))?;

    let mut refreshed = session.clone();
    refreshed.data.access_jwt = out.access_jwt;
    refreshed.data.refresh_jwt = out.refresh_jwt;
    Ok(refreshed)
}
