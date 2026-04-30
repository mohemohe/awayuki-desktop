//! Bluesky / AT Protocol REST rate-limit snapshot.
//!
//! Bluesky's PDS / AppView returns IETF-draft `RateLimit-*` headers on most
//! XRPC endpoints. We capture them on every response (see
//! `bluesky::xrpc::RateLimitTrackingClient`) and surface the latest snapshot
//! to the UI via `BlueskyClient::rate_limit()`.
//!
//! The snapshot is intentionally per-account (each `BlueskyClient` owns its
//! own `RateLimitState`) because Bluesky's limits are scoped to the
//! authenticated identity. The "last response wins" semantic is good enough
//! for an at-a-glance status display: each endpoint can have its own bucket,
//! but the user mainly cares whether they're close to running out, which is
//! a property of whichever bucket was most recently touched.

use std::sync::Arc;
use std::sync::RwLock;

use atrium_api::xrpc::http::HeaderMap;
use chrono::{DateTime, TimeZone, Utc};

/// Shared, lock-protected slot holding the latest rate-limit snapshot.
///
/// `None` means we haven't observed a response with `RateLimit-*` headers
/// yet — typically the case immediately after login, before any read fires.
pub type RateLimitState = Arc<RwLock<Option<RateLimitSnapshot>>>;

/// Construct a fresh shared rate-limit slot. Returned `Arc` is cheap to
/// clone — share one between the wrapping XRPC client and the
/// `BlueskyClient` that exposes it.
pub fn new_state() -> RateLimitState {
    Arc::new(RwLock::new(None))
}

#[derive(Debug, Clone)]
pub struct RateLimitSnapshot {
    /// Total budget for the current window (`RateLimit-Limit`).
    pub limit: u32,
    /// Remaining requests in the current window (`RateLimit-Remaining`).
    pub remaining: u32,
    /// When the window resets (`RateLimit-Reset`, parsed as Unix seconds).
    pub reset_at: DateTime<Utc>,
    /// Raw policy descriptor, e.g. `3000;w=300` (`RateLimit-Policy`).
    pub policy: Option<String>,
    /// Wall-clock time we observed this snapshot. Lets the UI show "as of N
    /// seconds ago" without polling the network just to refresh the display.
    pub observed_at: DateTime<Utc>,
}

impl RateLimitSnapshot {
    /// Parse `RateLimit-*` headers from an HTTP response. Returns `None` if
    /// the required headers (limit / remaining / reset) are missing or
    /// unparseable — Bluesky doesn't always include them (some legacy
    /// endpoints, error responses), and we'd rather keep the previous
    /// snapshot than overwrite it with bogus data.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let limit = parse_u32(headers, "ratelimit-limit")?;
        let remaining = parse_u32(headers, "ratelimit-remaining")?;
        let reset_unix = parse_i64(headers, "ratelimit-reset")?;
        let reset_at = Utc.timestamp_opt(reset_unix, 0).single()?;
        let policy = headers
            .get("ratelimit-policy")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Some(Self {
            limit,
            remaining,
            reset_at,
            policy,
            observed_at: Utc::now(),
        })
    }

    /// Fraction of the budget consumed (0.0 — 1.0). Useful for a progress bar.
    pub fn used_fraction(&self) -> f32 {
        if self.limit == 0 {
            return 0.0;
        }
        let used = self.limit.saturating_sub(self.remaining);
        (used as f32) / (self.limit as f32)
    }
}

fn parse_u32(headers: &HeaderMap, name: &str) -> Option<u32> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn parse_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}
