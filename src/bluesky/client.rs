//! Bluesky client wrapping `bsky-sdk`'s `BskyAgent`.
//!
//! Bluesky uses the AT Protocol. Authentication is via username (handle / DID / email)
//! and an app password, exchanged for an access JWT and a refresh JWT through
//! `com.atproto.server.createSession`. The agent transparently refreshes tokens.
//!
//! For persistence we serialise the entire `AtpSession` (which carries access/refresh JWTs,
//! DID, handle, etc.) as JSON and store it in `login_accounts.access_token`.

use std::sync::Arc;

use atrium_api::agent::atp_agent::AtpSession;
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

        agent
            .resume_session(stored.session)
            .await
            .map_err(|e| MastodonError::Other(format!("Bluesky resume_session failed: {}", e)))?;

        Ok(Self {
            agent: Arc::new(agent),
            domain: domain.to_string(),
            access_token: Arc::new(RwLock::new(access_token)),
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
