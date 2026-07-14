// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::ipc::dto::CreateSidecarWebviewRequest;
use crate::ipc::error::AppError;
use tauri::ipc::Request as IpcRequest;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn create_sidecar_webview(
    app: AppHandle,
    request: CreateSidecarWebviewRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command(
        "create_sidecar_webview",
        &ipc_request,
        desktop::create_sidecar_webview_impl(app, request),
    )
    .await
}

#[tauri::command]
pub(crate) fn navigate_sidecar_webview(
    app: AppHandle,
    sidecar_id: String,
    url: String,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command_sync(
        "navigate_sidecar_webview",
        &ipc_request,
        desktop::navigate_sidecar_webview_impl(app, sidecar_id, url),
    )
}

#[tauri::command]
pub(crate) fn reload_sidecar_webview(
    app: AppHandle,
    sidecar_id: String,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command_sync(
        "reload_sidecar_webview",
        &ipc_request,
        desktop::reload_sidecar_webview_impl(app, sidecar_id),
    )
}

#[tauri::command]
pub(crate) fn close_sidecar_webview(
    app: AppHandle,
    sidecar_id: String,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command_sync(
        "close_sidecar_webview",
        &ipc_request,
        desktop::close_sidecar_webview_impl(app, sidecar_id),
    )
}

#[tauri::command]
pub(crate) fn scroll_sidecar_webview_to_top(
    app: AppHandle,
    sidecar_id: String,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command_sync(
        "scroll_sidecar_webview_to_top",
        &ipc_request,
        desktop::scroll_sidecar_webview_to_top_impl(app, sidecar_id),
    )
}

#[tauri::command]
pub(crate) fn inject_sidecar_user_style(
    app: AppHandle,
    sidecar_id: String,
    user_style: String,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command_sync(
        "inject_sidecar_user_style",
        &ipc_request,
        desktop::inject_sidecar_user_style_impl(app, sidecar_id, user_style),
    )
}
