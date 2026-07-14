//! Release-package-only WebView security attestation and fixture activation.
//!
//! The environment-gated fixture emits diagnostics to stdout only. It never
//! persists state in SQLite, an OS store or a side file. Its loopback URL is
//! validated before it is injected into the main WebView.

use url::Url;

pub(super) fn emit_release_security_attestation(app: &tauri::App) {
    if std::env::var_os("AWAYUKI_RELEASE_SECURITY_SMOKE").is_none() {
        return;
    }
    let csp = app
        .config()
        .app
        .security
        .csp
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let csp_deny_default = [
        "base-uri 'none'",
        "object-src 'none'",
        "form-action 'none'",
        "frame-src 'none'",
    ]
    .iter()
    .all(|directive| csp.contains(directive));
    let csp_external_connect = csp
        .split(';')
        .find(|directive| directive.trim_start().starts_with("connect-src "))
        .is_some_and(|directive| {
            directive
                .split_whitespace()
                .any(|source| matches!(source, "http:" | "https:" | "ws:" | "wss:"))
        });
    let csp_remote_media = csp.contains("img-src 'self' http: https:")
        && csp.contains("media-src 'self' http: https:");
    println!(
        "AWAYUKI_RELEASE_SECURITY_REPORT release_build={} csp_deny_default={} csp_external_connect={} csp_remote_media={}",
        !cfg!(debug_assertions),
        csp_deny_default,
        csp_external_connect,
        csp_remote_media
    );
}

pub(super) fn inject_release_webview_smoke<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    let Some(raw_url) = std::env::var_os("AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL") else {
        return;
    };
    let Some(url) = validated_release_webview_smoke_url(&raw_url.to_string_lossy()) else {
        tracing::warn!("Ignoring non-loopback release WebView smoke URL");
        return;
    };
    let Ok(serialized_url) = serde_json::to_string(&url) else {
        return;
    };
    let script = format!(
        "window.__AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL__ = {serialized_url}; \
         window.dispatchEvent(new CustomEvent('awayuki-release-webview-smoke', \
         {{ detail: {serialized_url} }}));"
    );
    if let Err(error) = webview.eval(&script) {
        tracing::warn!(%error, "Failed to activate release WebView smoke fixture");
    }
}

pub(super) fn validated_release_webview_smoke_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "http" || url.port().is_none() {
        return None;
    }
    match url.host_str()? {
        "127.0.0.1" | "localhost" | "::1" => {
            Some(url.to_string().trim_end_matches('/').to_string())
        }
        _ => None,
    }
}
