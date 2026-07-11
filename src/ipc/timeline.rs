// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use tauri::State;

#[tauri::command]
pub(crate) async fn load_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::load_timeline_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("load_timeline", error))
}

#[tauri::command]
pub(crate) async fn load_more_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    desktop::load_more_timeline_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("load_more_timeline", error))
}

#[tauri::command]
pub(crate) async fn refresh_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::refresh_timeline_impl(state, request).await
}

#[tauri::command]
pub(crate) async fn air_context(
    state: State<'_, RuntimeState>,
    request: AirContextRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::air_context_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("air_context", error))
}

#[tauri::command]
pub(crate) async fn status_thread(
    state: State<'_, RuntimeState>,
    request: StatusThreadRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::status_thread_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("status_thread", error))
}
