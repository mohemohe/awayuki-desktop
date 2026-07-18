use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarOrigin {
    scheme: String,
    host: String,
    effective_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarPolicy {
    origin: SidecarOrigin,
}

impl SidecarPolicy {
    pub fn parse_initial_url(input: &str) -> Result<(Url, Self), String> {
        let url = Url::parse(input.trim())
            .map_err(|_| "Sidecar URL must be a valid http:// or https:// URL".to_string())?;
        let origin = SidecarOrigin::from_url(&url)?;
        Ok((url, Self { origin }))
    }

    pub fn allows_navigation(&self, url: &Url) -> bool {
        SidecarOrigin::from_url(url)
            .map(|origin| origin == self.origin)
            .unwrap_or(false)
    }

    pub fn should_open_external(&self, url: &Url) -> bool {
        SidecarOrigin::from_url(url)
            .map(|origin| origin != self.origin)
            .unwrap_or(false)
    }

    pub const fn allows_popup(&self, _url: &Url) -> bool {
        false
    }

    pub const fn allows_download(&self, _url: &Url) -> bool {
        false
    }
}

impl SidecarOrigin {
    fn from_url(url: &Url) -> Result<Self, String> {
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Sidecar URL must be a valid http:// or https:// URL".to_string());
        }
        let host = url
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| "Sidecar URL must include a host".to_string())?;
        let effective_port = url
            .port_or_known_default()
            .ok_or_else(|| "Sidecar URL must use a known HTTP port".to_string())?;
        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            effective_port,
        })
    }
}

#[derive(Debug, Clone)]
struct SidecarRuntimeEntry {
    policy: SidecarPolicy,
    user_style: String,
}

fn registry() -> &'static Mutex<HashMap<String, SidecarRuntimeEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, SidecarRuntimeEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a WebView lifecycle. Repeated create calls may update style but
/// cannot silently replace the origin of an already-open WebView.
pub fn register(label: &str, policy: SidecarPolicy, user_style: String) -> Result<(), String> {
    let mut entries = registry()
        .lock()
        .map_err(|_| "Sidecar lifecycle registry is unavailable".to_string())?;
    if let Some(existing) = entries.get_mut(label) {
        if existing.policy != policy {
            return Err("Close the existing sidecar before changing its origin".to_string());
        }
        existing.user_style = user_style;
        return Ok(());
    }
    entries.insert(
        label.to_string(),
        SidecarRuntimeEntry { policy, user_style },
    );
    Ok(())
}

pub fn policy(label: &str) -> Option<SidecarPolicy> {
    registry()
        .lock()
        .ok()
        .and_then(|entries| entries.get(label).map(|entry| entry.policy.clone()))
}

pub fn set_user_style(label: &str, user_style: String) -> Result<(), String> {
    let mut entries = registry()
        .lock()
        .map_err(|_| "Sidecar lifecycle registry is unavailable".to_string())?;
    let entry = entries
        .get_mut(label)
        .ok_or_else(|| format!("Sidecar lifecycle not found: {label}"))?;
    entry.user_style = user_style;
    Ok(())
}

pub fn user_style(label: &str) -> String {
    registry()
        .lock()
        .ok()
        .and_then(|entries| entries.get(label).map(|entry| entry.user_style.clone()))
        .unwrap_or_default()
}

/// Idempotent lifecycle cleanup used for close and failed creation.
pub fn remove(label: &str) {
    if let Ok(mut entries) = registry().lock() {
        entries.remove(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_is_limited_to_the_initial_effective_origin() {
        let (_, policy) = SidecarPolicy::parse_initial_url("https://Example.com/start").unwrap();
        for allowed in [
            "https://example.com/next",
            "https://example.com:443/explicit-default",
        ] {
            assert!(policy.allows_navigation(&Url::parse(allowed).unwrap()));
        }
        for denied in [
            "http://example.com/",
            "https://example.com:444/",
            "https://other.example/",
            "file:///tmp/secret",
            "data:text/html,unsafe",
        ] {
            assert!(!policy.allows_navigation(&Url::parse(denied).unwrap()));
        }
        for external in [
            "http://example.com/",
            "https://example.com:444/",
            "https://other.example/",
        ] {
            assert!(policy.should_open_external(&Url::parse(external).unwrap()));
        }
        for blocked in [
            "https://example.com/next",
            "file:///tmp/secret",
            "data:text/html,unsafe",
        ] {
            assert!(!policy.should_open_external(&Url::parse(blocked).unwrap()));
        }
        assert!(!policy.allows_popup(&Url::parse("https://example.com/popup").unwrap()));
        assert!(!policy.allows_download(&Url::parse("https://example.com/file").unwrap()));
    }

    #[test]
    fn create_reload_close_cycles_do_not_retain_policy_or_style() {
        let label = "sidecar-policy-cycle-test";
        remove(label);
        let (_, first) = SidecarPolicy::parse_initial_url("https://example.com/").unwrap();
        register(label, first.clone(), "body { color: red; }".into()).unwrap();
        // Reload/repeated create preserves the origin while allowing style updates.
        register(label, first, "body { color: blue; }".into()).unwrap();
        assert_eq!(user_style(label), "body { color: blue; }");

        let (_, other) = SidecarPolicy::parse_initial_url("https://other.example/").unwrap();
        assert!(register(label, other.clone(), String::new()).is_err());
        remove(label);
        remove(label);
        assert!(policy(label).is_none());
        assert!(user_style(label).is_empty());

        register(label, other, String::new()).unwrap();
        remove(label);
    }
}
