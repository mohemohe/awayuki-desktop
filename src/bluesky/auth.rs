//! Bluesky authentication via username + app password (`com.atproto.server.createSession`).
//!
//! Bluesky does not require a browser-based OAuth roundtrip for app passwords.
//! Users generate an app password in their Bluesky settings and we exchange it
//! for an `AtpSession` via `BskyAgent::login`.

use bsky_sdk::BskyAgent;

use crate::bluesky::client::{BlueskyClient, DEFAULT_BLUESKY_HOST};
use crate::bluesky::rate_limit;
use crate::bluesky::xrpc::RateLimitTrackingClient;
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
    // Plug in our rate-limit tracking client so that the very first request
    // (the createSession in `agent.login` below) already populates the
    // shared snapshot — by the time the user lands in the timeline the UI
    // has something to show.
    let rate_limit_state = rate_limit::new_state();
    let xrpc = RateLimitTrackingClient::new(
        format!("https://{}", DEFAULT_BLUESKY_HOST),
        rate_limit_state.clone(),
    );
    let agent = BskyAgent::builder()
        .client(xrpc)
        .build()
        .await
        .map_err(|e| MastodonError::Other(format!("Bluesky agent build failed: {}", e)))?;

    agent
        .login(identifier, password)
        .await
        .map_err(|e| MastodonError::Other(format!("Bluesky login failed: {}", e)))?;

    BlueskyClient::from_agent(
        domain,
        agent,
        rate_limit_state,
        streaming_url,
        Some(password.to_string()),
    )
    .await
}
