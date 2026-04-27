use std::time::Duration;

use crate::api::kind::ServerKind;
use crate::constants::APP_USER_AGENT;
use crate::mastodon::error::MastodonError;

/// Maximum time spent on each probe request.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Detect which fediverse server software runs on `domain`.
///
/// We try Mastodon-compatible probe (`GET /api/v2/instance`) first.
/// - If it returns a `version` field whose contents include `Paon/...`, we treat it as Paon.
/// - If it succeeds without that marker, we treat it as Mastodon.
/// - If it fails, we fall back to a Misskey probe (`POST /api/meta`).
///
/// `nodeinfo` is intentionally not consulted because some forks expose misleading or empty data.
pub async fn detect_server_kind(domain: &str) -> Result<ServerKind, MastodonError> {
    let http = reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .timeout(PROBE_TIMEOUT)
        .build()?;

    // 1. Try Mastodon / Paon
    let mastodon_url = format!("https://{}/api/v2/instance", domain);
    match http.get(&mastodon_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
                    if version.contains("Paon/") {
                        tracing::info!("Detected Paon server: {} ({})", domain, version);
                        return Ok(ServerKind::Paon);
                    }
                }
                tracing::info!("Detected Mastodon-compatible server: {}", domain);
                return Ok(ServerKind::Mastodon);
            }
        }
        Ok(resp) => {
            tracing::debug!(
                "Mastodon probe returned non-success status {} for {}",
                resp.status(),
                domain
            );
        }
        Err(e) => {
            tracing::debug!("Mastodon probe failed for {}: {}", domain, e);
        }
    }

    // 2. Try Misskey
    let misskey_url = format!("https://{}/api/meta", domain);
    let body = serde_json::json!({ "detail": false });
    match http.post(&misskey_url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!("Detected Misskey server: {}", domain);
            return Ok(ServerKind::Misskey);
        }
        Ok(resp) => {
            tracing::debug!(
                "Misskey probe returned non-success status {} for {}",
                resp.status(),
                domain
            );
        }
        Err(e) => {
            tracing::debug!("Misskey probe failed for {}: {}", domain, e);
        }
    }

    Err(MastodonError::IncompatibleInstance(format!(
        "Could not identify server software at {}",
        domain
    )))
}
