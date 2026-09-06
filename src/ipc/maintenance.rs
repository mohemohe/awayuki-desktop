// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::application::maintenance::{self, DbSummary, StatusBarSnapshot};
use crate::ipc::dto::{ExplainCustomTimelineRequest, IcuMatchExpressionRequest};
use crate::ipc::error::AppError;
use crate::observability::{DiagnosticsSnapshot, SupportBundle, SupportBundleRequest};
use tauri::ipc::Request as IpcRequest;
use tauri::State;

#[tauri::command]
pub(crate) fn get_web_socket_statuses(
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<crate::services::websocket_status::WebSocketStatus>, AppError> {
    desktop::observe_string_command_sync(
        "get_web_socket_statuses",
        &ipc_request,
        Ok(crate::services::websocket_status::snapshot()),
    )
}

#[tauri::command]
pub(crate) fn reconnect_web_socket(
    id: Option<String>,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command_sync(
        "reconnect_web_socket",
        &ipc_request,
        crate::services::websocket_status::reconnect(id.as_deref()),
    )
}

#[tauri::command]
pub(crate) async fn vacuum_database(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<DbSummary, AppError> {
    desktop::observe_string_command(
        "vacuum_database",
        &ipc_request,
        maintenance::vacuum_database(state),
    )
    .await
}

#[tauri::command]
pub(crate) async fn explain_custom_timeline(
    state: State<'_, RuntimeState>,
    request: ExplainCustomTimelineRequest,
) -> Result<Vec<crate::db::queries::custom_timeline::QueryPlanStep>, AppError> {
    maintenance::explain_custom_timeline(state, request).await
}

#[tauri::command]
pub(crate) fn icu_match_expression(
    request: IcuMatchExpressionRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<Option<String>, AppError> {
    desktop::observe_string_command_sync(
        "icu_match_expression",
        &ipc_request,
        Ok(maintenance::icu_match_expression(request)),
    )
}

#[tauri::command]
pub(crate) async fn clear_status_cache(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<DbSummary, AppError> {
    desktop::observe_string_command(
        "clear_status_cache",
        &ipc_request,
        maintenance::clear_status_cache(state),
    )
    .await
}

#[tauri::command]
pub(crate) async fn status_bar_snapshot(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<StatusBarSnapshot, AppError> {
    desktop::observe_string_command(
        "status_bar_snapshot",
        &ipc_request,
        maintenance::status_bar_snapshot(state),
    )
    .await
}

#[tauri::command]
pub(crate) async fn diagnostics_snapshot(
    ipc_request: IpcRequest<'_>,
) -> Result<DiagnosticsSnapshot, AppError> {
    let operation_id = desktop::ipc_operation_id(&ipc_request);
    Ok(desktop::observe_infallible_command(
        "diagnostics_snapshot",
        operation_id,
        maintenance::diagnostics_snapshot(),
    )
    .await)
}

#[tauri::command]
pub(crate) async fn support_bundle(
    state: State<'_, RuntimeState>,
    request: SupportBundleRequest,
) -> Result<SupportBundle, AppError> {
    maintenance::support_bundle(state, request).await
}
