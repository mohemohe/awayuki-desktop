use std::collections::BTreeMap;
use std::fmt::Display;

use serde::{Deserialize, Serialize};

/// Stable, machine-readable failures exposed across the IPC boundary.
///
/// Keep this enum deliberately small. Protocol and database adapters may have
/// much more detailed internal errors, but UI behaviour must not depend on
/// their wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppErrorCode {
    AuthenticationExpired,
    RateLimited,
    Timeout,
    Validation,
    DatabaseBusy,
    CapabilityUnsupported,
    IpcResponseLost,
    Internal,
}

impl AppErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationExpired => "authentication_expired",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::Validation => "validation",
            Self::DatabaseBusy => "database_busy",
            Self::CapabilityUnsupported => "capability_unsupported",
            Self::IpcResponseLost => "ipc_response_lost",
            Self::Internal => "internal",
        }
    }

    pub const fn message_key(self) -> &'static str {
        match self {
            Self::AuthenticationExpired => "errors.authentication_expired",
            Self::RateLimited => "errors.rate_limited",
            Self::Timeout => "errors.timeout",
            Self::Validation => "errors.validation",
            Self::DatabaseBusy => "errors.database_busy",
            Self::CapabilityUnsupported => "errors.capability_unsupported",
            Self::IpcResponseLost => "errors.ipc_response_lost",
            Self::Internal => "errors.internal",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Timeout | Self::DatabaseBusy | Self::IpcResponseLost
        )
    }
}

/// The only error shape commands should expose to the WebView.
///
/// `safe_details` is an allow-listed map for non-sensitive scalar metadata
/// such as a retry delay. Raw causes are logged after redaction and never
/// serialized into this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: AppErrorCode,
    pub message_key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub safe_details: BTreeMap<String, String>,
    pub retryable: bool,
    pub request_id: String,
}

impl AppError {
    pub fn new(code: AppErrorCode, request_id: impl Into<String>) -> Self {
        Self {
            code,
            message_key: code.message_key().to_string(),
            safe_details: BTreeMap::new(),
            retryable: code.retryable(),
            request_id: request_id.into(),
        }
    }

    pub fn from_source(source: impl Display, request_id: impl Into<String>) -> Self {
        let request_id = request_id.into();
        let raw = source.to_string();
        let code = classify_source_error(&raw);
        Self::from_classified_source(code, raw, request_id)
    }

    pub fn from_adapter(
        source: crate::domain::adapter_error::AdapterError,
        request_id: impl Into<String>,
    ) -> Self {
        use crate::domain::adapter_error::AdapterErrorCode;

        let request_id = request_id.into();
        let code = match source.code {
            AdapterErrorCode::Unauthorized => AppErrorCode::AuthenticationExpired,
            AdapterErrorCode::RateLimited => AppErrorCode::RateLimited,
            AdapterErrorCode::Timeout => AppErrorCode::Timeout,
            AdapterErrorCode::Unsupported => AppErrorCode::CapabilityUnsupported,
            AdapterErrorCode::InvalidResponse
            | AdapterErrorCode::Transport
            | AdapterErrorCode::Internal => AppErrorCode::Internal,
        };
        let retry_after = source.retry_after_seconds;
        let mut error = Self::from_classified_source(code, source.to_string(), request_id);
        if let Some(retry_after) = retry_after {
            error
                .safe_details
                .insert("retryAfterSeconds".to_string(), retry_after.to_string());
        }
        error
    }

    fn from_classified_source(code: AppErrorCode, raw: String, request_id: String) -> Self {
        tracing::warn!(
            request_id,
            error_code = ?code,
            cause = %crate::observability::redact_text(&raw),
            "IPC command failed"
        );
        Self::new(code, request_id)
    }

    pub fn validation(request_id: impl Into<String>) -> Self {
        Self::new(AppErrorCode::Validation, request_id)
    }
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} ({})", self.message_key, self.request_id)
    }
}

impl std::error::Error for AppError {}

pub fn classify_source_error(raw: &str) -> AppErrorCode {
    let normalized = raw.to_ascii_lowercase();
    if normalized.contains("rate limit") || normalized.contains("http 429") {
        AppErrorCode::RateLimited
    } else if normalized.contains("timed out") || normalized.contains("timeout") {
        AppErrorCode::Timeout
    } else if normalized.contains("database is locked")
        || normalized.contains("database busy")
        || normalized.contains("sqlite_busy")
    {
        AppErrorCode::DatabaseBusy
    } else if normalized.contains("unauthorized")
        || normalized.contains("http 401")
        || normalized.contains("token expired")
        || normalized.contains("session not found")
        || normalized.contains("not signed in")
    {
        AppErrorCode::AuthenticationExpired
    } else if normalized.contains("not supported") || normalized.contains("unsupported capability")
    {
        AppErrorCode::CapabilityUnsupported
    } else if normalized.contains("invalid")
        || normalized.contains("is required")
        || normalized.contains("is empty")
        || normalized.contains("must ")
        || normalized.contains("duplicate ")
    {
        AppErrorCode::Validation
    } else {
        AppErrorCode::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_stable_error_categories() {
        assert_eq!(
            classify_source_error("HTTP 401 Unauthorized"),
            AppErrorCode::AuthenticationExpired
        );
        assert_eq!(
            classify_source_error("rate limit exceeded"),
            AppErrorCode::RateLimited
        );
        assert_eq!(
            classify_source_error("request timed out"),
            AppErrorCode::Timeout
        );
        assert_eq!(
            classify_source_error("status is required"),
            AppErrorCode::Validation
        );
        assert_eq!(
            classify_source_error("database is locked"),
            AppErrorCode::DatabaseBusy
        );
        assert_eq!(
            classify_source_error("SQL syntax near token"),
            AppErrorCode::Internal
        );
    }

    #[test]
    fn serialized_error_never_contains_the_internal_cause() {
        let error = AppError::from_source(
            "SQL error with token=secret at /Users/alice/private.db",
            "request-1",
        );
        let json = serde_json::to_string(&error).expect("serialize app error");
        assert!(!json.contains("secret"));
        assert!(!json.contains("/Users"));
        assert!(!json.contains("SQL"));
        assert!(json.contains("internal"));
        assert!(json.contains("request-1"));
    }

    #[test]
    fn maps_typed_api_errors_without_depending_on_display_text() {
        use crate::domain::adapter_error::{AdapterError, AdapterErrorCode, AdapterSource};

        let unauthorized = AppError::from_adapter(
            AdapterError::new(
                AdapterErrorCode::Unauthorized,
                AdapterSource::AtProto,
                None,
                crate::mastodon::error::MastodonError::Unauthorized,
            ),
            "request-2",
        );
        assert_eq!(unauthorized.code, AppErrorCode::AuthenticationExpired);

        let limited = AppError::from_adapter(
            AdapterError::new(
                AdapterErrorCode::RateLimited,
                AdapterSource::ActivityPub,
                Some(42),
                crate::mastodon::error::MastodonError::RateLimited {
                    retry_after: Some(42),
                },
            ),
            "request-3",
        );
        assert_eq!(limited.code, AppErrorCode::RateLimited);
        assert_eq!(
            limited
                .safe_details
                .get("retryAfterSeconds")
                .map(String::as_str),
            Some("42")
        );
    }
}
