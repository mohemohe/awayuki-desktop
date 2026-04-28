//! Bluesky authentication via username + app password (`com.atproto.server.createSession`).
//!
//! Bluesky does not require a browser-based OAuth roundtrip for app passwords.
//! Users generate an app password in their Bluesky settings and we exchange it
//! for an `AtpSession` via `BskyAgent::login`.

use bsky_sdk::BskyAgent;

use crate::bluesky::client::BlueskyClient;
use crate::mastodon::error::MastodonError;

/// Log into Bluesky using identifier (handle / DID / email) and app password.
///
/// Returns a ready-to-use `BlueskyClient` whose stored token contains the full
/// `AtpSession` (access JWT, refresh JWT, DID, handle).
pub async fn login_with_app_password(
    domain: &str,
    identifier: &str,
    password: &str,
    streaming_url: String,
) -> Result<BlueskyClient, MastodonError> {
    let agent = BskyAgent::builder()
        .build()
        .await
        .map_err(|e| MastodonError::Other(format!("Bluesky agent build failed: {}", e)))?;

    agent
        .login(identifier, password)
        .await
        .map_err(|e| MastodonError::Other(format!("Bluesky login failed: {}", e)))?;

    BlueskyClient::from_agent(domain, agent, streaming_url).await
}
