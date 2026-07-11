// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use crate::observability::{DiagnosticsSnapshot, SupportBundle, SupportBundleRequest};
use tauri::State;

#[tauri::command]
pub(crate) async fn vacuum_database(state: State<'_, RuntimeState>) -> Result<DbSummary, AppError> {
    desktop::vacuum_database_impl(state)
        .await
        .map_err(|error| desktop::command_error("vacuum_database", error))
}

#[tauri::command]
pub(crate) async fn explain_custom_timeline(
    state: State<'_, RuntimeState>,
    request: ExplainCustomTimelineRequest,
) -> Result<Vec<crate::db::queries::custom_timeline::QueryPlanStep>, AppError> {
    desktop::explain_custom_timeline_impl(state, request).await
}

#[tauri::command]
pub(crate) async fn clear_status_cache(
    state: State<'_, RuntimeState>,
) -> Result<DbSummary, AppError> {
    desktop::clear_status_cache_impl(state)
        .await
        .map_err(|error| desktop::command_error("clear_status_cache", error))
}

#[tauri::command]
pub(crate) async fn status_bar_snapshot(
    state: State<'_, RuntimeState>,
) -> Result<StatusBarSnapshot, AppError> {
    desktop::status_bar_snapshot_impl(state)
        .await
        .map_err(|error| desktop::command_error("status_bar_snapshot", error))
}

#[tauri::command]
pub(crate) async fn diagnostics_snapshot() -> DiagnosticsSnapshot {
    desktop::diagnostics_snapshot_impl().await
}

#[tauri::command]
pub(crate) async fn support_bundle(
    state: State<'_, RuntimeState>,
    request: SupportBundleRequest,
) -> Result<SupportBundle, AppError> {
    desktop::support_bundle_impl(state, request).await
}
