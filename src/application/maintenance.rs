//! Portable database maintenance and in-memory diagnostics use cases.

use chrono::Utc;
use serde::Serialize;
use tauri::State;
use tokio_util::sync::CancellationToken;

use crate::application::desktop::RuntimeState;
use crate::constants::APP_VERSION;
use crate::db::queries::{custom_timeline, settings};
use crate::ipc::dto::{ExplainCustomTimelineRequest, IcuMatchExpressionRequest};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::observability::{
    DiagnosticsSnapshot, OperationContext, SupportBundle, SupportBundleRequest,
};
use crate::state::paths;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DbSummary {
    pub(crate) path: String,
    pub(crate) size: String,
    pub(crate) status_count: i64,
    pub(crate) recent_status_count: i64,
    pub(crate) account_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusBarSnapshot {
    pub(crate) status_count: i64,
    pub(crate) recent_status_count: i64,
    pub(crate) uptime_seconds: u64,
}

pub(crate) async fn vacuum_database(state: State<'_, RuntimeState>) -> Result<DbSummary, String> {
    settings::vacuum(state.database().writer())
        .await
        .map_err(|error| error.to_string())?;
    database_summary(&state).await
}

pub(crate) async fn explain_custom_timeline(
    state: State<'_, RuntimeState>,
    request: ExplainCustomTimelineRequest,
) -> Result<Vec<custom_timeline::QueryPlanStep>, AppError> {
    let mut operation = OperationContext::start(
        "explain_custom_timeline",
        request.operation_id.as_deref(),
        None,
    );
    operation.phase("db");
    if let Err(error) = state.startup_gate().wait_until_ready().await {
        return Err(operation.finish_error(error));
    }
    match custom_timeline::explain(
        state.database().analytics_reader(),
        &request.sql,
        &CancellationToken::new(),
    )
    .await
    {
        Ok(plan) => {
            operation.finish_ok();
            Ok(plan)
        }
        Err(error) => {
            use custom_timeline::CustomTimelineError;

            let position = error.query_position(&request.sql);
            let code = match &error {
                CustomTimelineError::Invalid { .. }
                | CustomTimelineError::Rejected(_)
                | CustomTimelineError::SqlTooLarge
                | CustomTimelineError::ResultTooLarge => AppErrorCode::Validation,
                CustomTimelineError::ExecutionBudget => AppErrorCode::Timeout,
                CustomTimelineError::Cancelled => AppErrorCode::Cancelled,
                CustomTimelineError::Encoding(_) | CustomTimelineError::Connection(_) => {
                    AppErrorCode::Internal
                }
            };
            let mut app_error = AppError::from_code(code, error, operation.id());
            if let Some((line, column)) = position {
                app_error = app_error
                    .with_safe_detail("line", line)
                    .with_safe_detail("column", column);
            }
            Err(operation.finish_app_error(app_error))
        }
    }
}

pub(crate) fn icu_match_expression(request: IcuMatchExpressionRequest) -> Option<String> {
    crate::db::icu_search::match_expression(&request.term)
}

pub(crate) async fn clear_status_cache(
    state: State<'_, RuntimeState>,
) -> Result<DbSummary, String> {
    settings::clear_status_cache(state.database().writer())
        .await
        .map_err(|error| error.to_string())?;
    database_summary(&state).await
}

pub(crate) async fn status_bar_snapshot(
    state: State<'_, RuntimeState>,
) -> Result<StatusBarSnapshot, String> {
    let status_count = settings::get_status_count(state.database().reader())
        .await
        .unwrap_or_default();
    let recent_since = (Utc::now() - chrono::Duration::minutes(15)).to_rfc3339();
    let recent_status_count =
        settings::get_recent_status_count(state.database().reader(), &recent_since)
            .await
            .unwrap_or_default();

    Ok(StatusBarSnapshot {
        status_count,
        recent_status_count,
        uptime_seconds: state.uptime_seconds(),
    })
}

pub(crate) async fn diagnostics_snapshot() -> DiagnosticsSnapshot {
    crate::observability::snapshot()
}

/// Build an explicitly requested, in-memory support payload. It is returned to
/// the caller and is never persisted outside the portable SQLite database.
pub(crate) async fn support_bundle(
    state: State<'_, RuntimeState>,
    request: SupportBundleRequest,
) -> Result<SupportBundle, AppError> {
    let mut operation =
        OperationContext::start("support_bundle", request.operation_id.as_deref(), None);
    operation.phase("db");
    let query_started_at = std::time::Instant::now();
    let schema_version =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(state.database().reader())
            .await;
    crate::observability::observe_db_query(1, elapsed_ms(query_started_at));
    match schema_version {
        Ok(schema_version) => {
            let bundle = SupportBundle::in_memory(APP_VERSION, schema_version, request.frontend);
            operation.finish_ok();
            Ok(bundle)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn database_summary(state: &RuntimeState) -> Result<DbSummary, String> {
    let status_count = settings::get_status_count(state.database().reader())
        .await
        .unwrap_or_default();
    let recent_since = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let recent_status_count =
        settings::get_recent_status_count(state.database().reader(), &recent_since)
            .await
            .unwrap_or_default();
    let account_count = settings::get_account_count(state.database().reader())
        .await
        .unwrap_or_default();
    let size = settings::get_db_size(state.database().reader())
        .await
        .unwrap_or_default();

    Ok(DbSummary {
        path: paths::db_path().display().to_string(),
        size: format_size(size),
        status_count,
        recent_status_count,
        account_count,
    })
}

fn elapsed_ms(started_at: std::time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn format_size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    if bytes >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", bytes / 1024.0 / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{} B", bytes as i64)
    }
}
