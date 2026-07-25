use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ipc::error::{AppError, AppErrorCode};

const EVENT_CAPACITY: usize = 256;
const SUPPORT_BUNDLE_SCHEMA_VERSION: u32 = 1;

static HEALTH: OnceLock<HealthRegistry> = OnceLock::new();
static SECRET_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
static OAUTH_QUERY: OnceLock<Regex> = OnceLock::new();
static UNIX_PATH: OnceLock<Regex> = OnceLock::new();
static WINDOWS_PATH: OnceLock<Regex> = OnceLock::new();

struct HealthRegistry {
    process_salt: String,
    active_operations: AtomicU64,
    completed_operations: AtomicU64,
    failed_operations: AtomicU64,
    api_requests: AtomicU64,
    http_retries: AtomicU64,
    rate_limited_errors: AtomicU64,
    db_transactions: AtomicU64,
    db_statements: AtomicU64,
    db_rows: AtomicU64,
    db_query_duration_ms: AtomicU64,
    db_busy_errors: AtomicU64,
    stream_queue_depth: AtomicU64,
    stream_max_queue_depth: AtomicU64,
    stream_coalesced: AtomicU64,
    stream_dropped: AtomicU64,
    stream_resyncs: AtomicU64,
    stream_resync_required: AtomicBool,
    cache_entries: AtomicU64,
    events: Mutex<VecDeque<DiagnosticEvent>>,
}

impl HealthRegistry {
    fn new() -> Self {
        Self {
            process_salt: uuid::Uuid::new_v4().to_string(),
            active_operations: AtomicU64::new(0),
            completed_operations: AtomicU64::new(0),
            failed_operations: AtomicU64::new(0),
            api_requests: AtomicU64::new(0),
            http_retries: AtomicU64::new(0),
            rate_limited_errors: AtomicU64::new(0),
            db_transactions: AtomicU64::new(0),
            db_statements: AtomicU64::new(0),
            db_rows: AtomicU64::new(0),
            db_query_duration_ms: AtomicU64::new(0),
            db_busy_errors: AtomicU64::new(0),
            stream_queue_depth: AtomicU64::new(0),
            stream_max_queue_depth: AtomicU64::new(0),
            stream_coalesced: AtomicU64::new(0),
            stream_dropped: AtomicU64::new(0),
            stream_resyncs: AtomicU64::new(0),
            stream_resync_required: AtomicBool::new(false),
            cache_entries: AtomicU64::new(0),
            events: Mutex::new(VecDeque::with_capacity(EVENT_CAPACITY)),
        }
    }

    fn push(&self, event: DiagnosticEvent) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if events.len() == EVENT_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }
}

fn health() -> &'static HealthRegistry {
    HEALTH.get_or_init(HealthRegistry::new)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub at: String,
    pub operation_id: String,
    pub command: String,
    pub phase: String,
    pub duration_ms: u64,
    pub result_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StreamHealthSnapshot {
    pub queue_depth: u64,
    pub max_queue_depth: u64,
    pub coalesced: u64,
    pub dropped: u64,
    pub resyncs: u64,
    pub resync_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSnapshot {
    pub schema_version: u32,
    pub active_operations: u64,
    pub completed_operations: u64,
    pub failed_operations: u64,
    pub api_requests: u64,
    pub http_retries: u64,
    pub rate_limited_errors: u64,
    pub db_transactions: u64,
    pub db_statements: u64,
    pub db_rows: u64,
    pub db_query_duration_ms: u64,
    pub db_busy_errors: u64,
    pub cache_entries: u64,
    pub stream: StreamHealthSnapshot,
    pub dropped_log_records: u64,
    pub rolling_event_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendStartupMetrics {
    pub module_evaluated_ms: u64,
    pub last_initial_script_response_ms: u64,
    pub parse_evaluate_after_script_ms: u64,
    pub dom_interactive_ms: u64,
    pub first_react_commit_ms: u64,
    pub first_interactive_ms: u64,
    pub js_heap_used_bytes: u64,
    pub js_heap_limit_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendRenderMetricSnapshot {
    pub commits: u64,
    pub sample_count: u64,
    pub average_duration_ms: u64,
    pub p95_duration_ms: u64,
    pub last_duration_ms: u64,
    pub frame_sample_count: u64,
    pub frame_average_duration_ms: u64,
    pub frame_p95_duration_ms: u64,
    pub last_frame_duration_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendRenderMetricsSnapshot {
    pub timeline_stream: FrontendRenderMetricSnapshot,
    pub timeline_scroll: FrontendRenderMetricSnapshot,
    pub profile_open: FrontendRenderMetricSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendHealthSnapshot {
    pub active_operations: u64,
    pub completed_operations: u64,
    pub failed_operations: u64,
    pub stream_sequence_gaps: u64,
    pub stream_resyncs: u64,
    pub pending_stream_events: u64,
    #[serde(default)]
    pub startup: FrontendStartupMetrics,
    #[serde(default)]
    pub render: FrontendRenderMetricsSnapshot,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupportBundleRequest {
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub frontend: FrontendHealthSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleEnvironment {
    pub app_version: String,
    pub database_schema_version: i64,
    pub persistence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundle {
    pub schema_version: u32,
    pub generated_at: String,
    pub environment: SupportBundleEnvironment,
    pub backend: DiagnosticsSnapshot,
    pub frontend: FrontendHealthSnapshot,
    pub recent_events: Vec<DiagnosticEvent>,
}

impl SupportBundle {
    pub fn in_memory(
        app_version: &str,
        database_schema_version: i64,
        frontend: FrontendHealthSnapshot,
    ) -> Self {
        Self {
            schema_version: SUPPORT_BUNDLE_SCHEMA_VERSION,
            generated_at: Utc::now().to_rfc3339(),
            environment: SupportBundleEnvironment {
                app_version: app_version.to_string(),
                database_schema_version,
                persistence: "sqlite_only_portable".to_string(),
            },
            backend: snapshot(),
            frontend,
            recent_events: recent_events(),
        }
    }
}

/// A single UI intent. The client-provided UUID is accepted only when valid;
/// malformed values never become log fields.
pub struct OperationContext {
    operation_id: String,
    command: &'static str,
    account_id: Option<String>,
    started_at: Instant,
    finished: bool,
}

impl OperationContext {
    pub fn start(command: &'static str, requested_id: Option<&str>, account: Option<&str>) -> Self {
        let registry = health();
        let operation_id = requested_id
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .map(|value| value.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let account_id = account.map(anonymize_account);
        registry.active_operations.fetch_add(1, Ordering::Relaxed);
        registry.push(DiagnosticEvent {
            at: Utc::now().to_rfc3339(),
            operation_id: operation_id.clone(),
            command: command.to_string(),
            phase: "ipc".to_string(),
            duration_ms: 0,
            result_code: "started".to_string(),
            account_id: account_id.clone(),
            metrics: BTreeMap::new(),
        });
        tracing::info!(
            operation_id,
            command,
            account_id,
            phase = "ipc",
            "operation started"
        );
        Self {
            operation_id,
            command,
            account_id,
            started_at: Instant::now(),
            finished: false,
        }
    }

    pub fn id(&self) -> &str {
        &self.operation_id
    }

    pub fn phase(&self, phase: &'static str) {
        let registry = health();
        match phase {
            "api" => {
                registry.api_requests.fetch_add(1, Ordering::Relaxed);
            }
            "db" | "commit" => {
                registry.db_transactions.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        registry.push(DiagnosticEvent {
            at: Utc::now().to_rfc3339(),
            operation_id: self.operation_id.clone(),
            command: self.command.to_string(),
            phase: phase.to_string(),
            duration_ms: elapsed_ms(self.started_at),
            result_code: "progress".to_string(),
            account_id: self.account_id.clone(),
            metrics: BTreeMap::new(),
        });
        tracing::info!(
            operation_id = self.operation_id,
            command = self.command,
            account_id = self.account_id,
            phase,
            duration_ms = elapsed_ms(self.started_at),
            "operation phase"
        );
    }

    pub fn finish_ok(&mut self) {
        self.finish(AppErrorCode::Internal, "ok");
    }

    pub fn finish_error(&mut self, source: impl std::fmt::Display) -> AppError {
        let error = AppError::from_source(source, self.operation_id.clone());
        self.finish_app_error(error)
    }

    pub fn finish_error_code(
        &mut self,
        code: AppErrorCode,
        source: impl std::fmt::Display,
    ) -> AppError {
        let error = AppError::from_code(code, source, self.operation_id.clone());
        self.finish_app_error(error)
    }

    pub fn finish_app_error(&mut self, error: AppError) -> AppError {
        self.finish(error.code, error.code.as_str());
        error
    }

    fn finish(&mut self, code: AppErrorCode, result: &str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let registry = health();
        registry.active_operations.fetch_sub(1, Ordering::Relaxed);
        if result == "ok" {
            registry
                .completed_operations
                .fetch_add(1, Ordering::Relaxed);
        } else {
            registry.failed_operations.fetch_add(1, Ordering::Relaxed);
            if code == AppErrorCode::DatabaseBusy {
                registry.db_busy_errors.fetch_add(1, Ordering::Relaxed);
            }
            if code == AppErrorCode::RateLimited {
                registry.rate_limited_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        registry.push(DiagnosticEvent {
            at: Utc::now().to_rfc3339(),
            operation_id: self.operation_id.clone(),
            command: self.command.to_string(),
            phase: "complete".to_string(),
            duration_ms: elapsed_ms(self.started_at),
            result_code: result.to_string(),
            account_id: self.account_id.clone(),
            metrics: BTreeMap::new(),
        });
        tracing::info!(
            operation_id = self.operation_id,
            command = self.command,
            account_id = self.account_id,
            result_code = result,
            duration_ms = elapsed_ms(self.started_at),
            "operation completed"
        );
    }
}

impl Drop for OperationContext {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(AppErrorCode::Internal, "cancelled");
        }
    }
}

pub fn observe_stream_batch(queue_depth: usize, coalesced: usize, dropped: usize) {
    let registry = health();
    let depth = queue_depth as u64;
    registry.stream_queue_depth.store(depth, Ordering::Relaxed);
    registry
        .stream_max_queue_depth
        .fetch_max(depth, Ordering::Relaxed);
    registry
        .stream_coalesced
        .fetch_add(coalesced as u64, Ordering::Relaxed);
    registry
        .stream_dropped
        .fetch_add(dropped as u64, Ordering::Relaxed);
    if queue_depth > 0 || coalesced > 0 || dropped > 0 {
        registry.push(DiagnosticEvent {
            at: Utc::now().to_rfc3339(),
            operation_id: "stream".to_string(),
            command: "timeline_stream".to_string(),
            phase: "queue".to_string(),
            duration_ms: 0,
            result_code: if dropped > 0 { "degraded" } else { "ok" }.to_string(),
            account_id: None,
            metrics: BTreeMap::from([
                ("queue_depth".to_string(), queue_depth as u64),
                ("coalesced".to_string(), coalesced as u64),
                ("dropped".to_string(), dropped as u64),
            ]),
        });
    }
}

pub fn observe_stream_resync() {
    let registry = health();
    registry.stream_resyncs.fetch_add(1, Ordering::Relaxed);
    registry
        .stream_resync_required
        .store(true, Ordering::Relaxed);
    registry.push(DiagnosticEvent {
        at: Utc::now().to_rfc3339(),
        operation_id: "stream".to_string(),
        command: "timeline_stream".to_string(),
        phase: "resync".to_string(),
        duration_ms: 0,
        result_code: "required".to_string(),
        account_id: None,
        metrics: BTreeMap::new(),
    });
}

pub fn observe_http_retry() {
    health().http_retries.fetch_add(1, Ordering::Relaxed);
}

pub fn observe_db_query(rows: usize, duration_ms: u64) {
    let registry = health();
    registry.db_statements.fetch_add(1, Ordering::Relaxed);
    registry.db_rows.fetch_add(rows as u64, Ordering::Relaxed);
    registry
        .db_query_duration_ms
        .fetch_add(duration_ms, Ordering::Relaxed);
}

pub fn observe_startup_sync(api_requests: u64, db_writes: u64, ready_ms: u64) {
    let registry = health();
    registry
        .api_requests
        .fetch_add(api_requests, Ordering::Relaxed);
    registry
        .db_transactions
        .fetch_add(db_writes, Ordering::Relaxed);
    registry
        .db_statements
        .fetch_add(db_writes, Ordering::Relaxed);
    registry.push(DiagnosticEvent {
        at: Utc::now().to_rfc3339(),
        operation_id: "startup-sync-metrics".to_string(),
        command: "startup_sync".to_string(),
        phase: "ready".to_string(),
        duration_ms: ready_ms,
        result_code: "ok".to_string(),
        account_id: None,
        metrics: BTreeMap::from([
            ("api_requests".to_string(), api_requests),
            ("db_writes".to_string(), db_writes),
            ("ready_ms".to_string(), ready_ms),
        ]),
    });
}

pub fn set_cache_entries(entries: usize) {
    health()
        .cache_entries
        .store(entries as u64, Ordering::Relaxed);
}

pub fn snapshot() -> DiagnosticsSnapshot {
    let registry = health();
    let rolling_event_count = registry
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .len();
    DiagnosticsSnapshot {
        schema_version: SUPPORT_BUNDLE_SCHEMA_VERSION,
        active_operations: registry.active_operations.load(Ordering::Relaxed),
        completed_operations: registry.completed_operations.load(Ordering::Relaxed),
        failed_operations: registry.failed_operations.load(Ordering::Relaxed),
        api_requests: registry.api_requests.load(Ordering::Relaxed),
        http_retries: registry.http_retries.load(Ordering::Relaxed),
        rate_limited_errors: registry.rate_limited_errors.load(Ordering::Relaxed),
        db_transactions: registry.db_transactions.load(Ordering::Relaxed),
        db_statements: registry.db_statements.load(Ordering::Relaxed),
        db_rows: registry.db_rows.load(Ordering::Relaxed),
        db_query_duration_ms: registry.db_query_duration_ms.load(Ordering::Relaxed),
        db_busy_errors: registry.db_busy_errors.load(Ordering::Relaxed),
        cache_entries: registry.cache_entries.load(Ordering::Relaxed),
        stream: StreamHealthSnapshot {
            queue_depth: registry.stream_queue_depth.load(Ordering::Relaxed),
            max_queue_depth: registry.stream_max_queue_depth.load(Ordering::Relaxed),
            coalesced: registry.stream_coalesced.load(Ordering::Relaxed),
            dropped: registry.stream_dropped.load(Ordering::Relaxed),
            resyncs: registry.stream_resyncs.load(Ordering::Relaxed),
            resync_required: registry.stream_resync_required.load(Ordering::Relaxed),
        },
        dropped_log_records: crate::state::logging::dropped_records(),
        rolling_event_count,
    }
}

fn recent_events() -> Vec<DiagnosticEvent> {
    health()
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .cloned()
        .collect()
}

fn anonymize_account(account: &str) -> String {
    let registry = health();
    let mut digest = Sha256::new();
    digest.update(registry.process_salt.as_bytes());
    digest.update(account.as_bytes());
    let digest = digest.finalize();
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hash, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("account:{}", &hash[..12])
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// Defense-in-depth for causes written to local developer logs. Support
/// bundles contain only structured events and never call this with post bodies.
pub(crate) fn redact_text(raw: &str) -> String {
    let assignment = SECRET_ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(access[_-]?token|refresh[_-]?token|password|client[_-]?secret|credential|oauth[_-]?(?:code|state)|authorization)(\s*[:=]\s*)([^\s&,\"'}]+)"#,
        )
        .expect("valid secret redaction expression")
    });
    let oauth_query = OAUTH_QUERY.get_or_init(|| {
        Regex::new(r"(?i)([?&](?:code|state|token|password)=)[^&#\s]+")
            .expect("valid OAuth query expression")
    });
    let unix_path = UNIX_PATH.get_or_init(|| {
        Regex::new(r"(?:/Users|/home|/var|/tmp|/private)/[^\s,;]+")
            .expect("valid Unix path expression")
    });
    let windows_path = WINDOWS_PATH.get_or_init(|| {
        Regex::new(r"(?i)\b[A-Z]:\\[^\s,;]+").expect("valid Windows path expression")
    });
    let text = assignment.replace_all(raw, "$1$2[redacted]");
    let text = oauth_query.replace_all(&text, "$1[redacted]");
    let text = unix_path.replace_all(&text, "[local-path]");
    windows_path.replace_all(&text, "[local-path]").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_fixture_removes_credentials_queries_and_paths() {
        let raw = "access_token=hunter2 https://example.test/cb?code=oauth-code&state=secret /Users/alice/Awayuki/awayuki.db C:\\Users\\alice\\awayuki.db";
        let redacted = redact_text(raw);
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("oauth-code"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("alice"));
        assert!(redacted.contains("[redacted]"));
        assert!(redacted.contains("[local-path]"));
    }

    #[test]
    fn support_bundle_snapshot_has_only_the_safe_environment_subset() {
        let bundle = SupportBundle::in_memory(
            "0.0.0-test",
            21,
            FrontendHealthSnapshot {
                completed_operations: 2,
                ..Default::default()
            },
        );
        let value = serde_json::to_value(bundle).expect("serialize support bundle");
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["environment"]["persistence"], "sqlite_only_portable");
        assert_eq!(value["environment"]["databaseSchemaVersion"], 21);
        assert!(value["environment"].get("databasePath").is_none());
        let json = value.to_string();
        assert!(!json.contains("access_token"));
        assert!(!json.contains("password"));
        assert!(!json.contains("/Users/"));
    }

    #[test]
    fn malformed_client_operation_ids_are_not_logged_verbatim() {
        let operation = OperationContext::start("test", Some("token=secret"), Some("user@test"));
        assert_ne!(operation.id(), "token=secret");
        assert!(uuid::Uuid::parse_str(operation.id()).is_ok());
        drop(operation);
        let events = serde_json::to_string(&recent_events()).expect("serialize events");
        assert!(!events.contains("token=secret"));
        assert!(!events.contains("user@test"));
        assert!(events.contains("account:"));
    }
}
