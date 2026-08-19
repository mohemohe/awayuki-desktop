// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::application::timeline;
use crate::ipc::dto::{
    AirContextRequest, CancelQuoteConsumerRequest, CancelTimelineQueryRequest, StatusThreadRequest,
    StatusViewerStatesRequest, TimelineRequest,
};
use crate::ipc::error::AppError;
use tauri::ipc::Request as IpcRequest;
use tauri::State;

#[tauri::command]
pub(crate) async fn load_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    timeline::load_timeline(state, request).await
}

#[tauri::command]
pub(crate) async fn load_more_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    timeline::load_more_timeline(state, request).await
}

#[tauri::command]
pub(crate) async fn load_timeline_gap(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    timeline::load_timeline_gap(state, request).await
}

#[tauri::command]
pub(crate) async fn cancel_timeline_query(
    state: State<'_, RuntimeState>,
    request: CancelTimelineQueryRequest,
) -> Result<bool, AppError> {
    timeline::cancel_timeline_query(state, request).await
}

#[tauri::command]
pub(crate) async fn cancel_quote_consumer(
    request: CancelQuoteConsumerRequest,
) -> Result<bool, AppError> {
    timeline::cancel_quote_consumer(request).await
}

#[tauri::command]
pub(crate) async fn refresh_timeline(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, AppError> {
    timeline::refresh_timeline(state, request).await
}

#[tauri::command]
pub(crate) async fn status_viewer_states(
    state: State<'_, RuntimeState>,
    request: StatusViewerStatesRequest,
) -> Result<Vec<StatusViewerStateSummary>, AppError> {
    timeline::status_viewer_states(state, request).await
}

#[tauri::command]
pub(crate) async fn air_context(
    state: State<'_, RuntimeState>,
    request: AirContextRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::observe_string_command(
        "air_context",
        &ipc_request,
        timeline::air_context(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn status_thread(
    state: State<'_, RuntimeState>,
    request: StatusThreadRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::observe_string_command(
        "status_thread",
        &ipc_request,
        timeline::status_thread(state, request),
    )
    .await
}
