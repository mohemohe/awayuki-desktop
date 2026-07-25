pub mod client;
pub mod endpoints;
pub mod error;
pub mod oauth;
pub mod streaming;
pub mod types;

/// Minimal scopes required by Awayuki's implemented Mastodon operations.
pub const OAUTH_SCOPES: &str = "read write follow";
