// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn create_sidecar_webview(
    app: AppHandle,
    request: CreateSidecarWebviewRequest,
) -> Result<(), AppError> {
    desktop::create_sidecar_webview_impl(app, request)
        .await
        .map_err(|error| desktop::command_error("create_sidecar_webview", error))
}

#[tauri::command]
pub(crate) fn navigate_sidecar_webview(
    app: AppHandle,
    sidecar_id: String,
    url: String,
) -> Result<(), AppError> {
    desktop::navigate_sidecar_webview_impl(app, sidecar_id, url)
        .map_err(|error| desktop::command_error("navigate_sidecar_webview", error))
}

#[tauri::command]
pub(crate) fn reload_sidecar_webview(app: AppHandle, sidecar_id: String) -> Result<(), AppError> {
    desktop::reload_sidecar_webview_impl(app, sidecar_id)
        .map_err(|error| desktop::command_error("reload_sidecar_webview", error))
}

#[tauri::command]
pub(crate) fn close_sidecar_webview(app: AppHandle, sidecar_id: String) -> Result<(), AppError> {
    desktop::close_sidecar_webview_impl(app, sidecar_id)
        .map_err(|error| desktop::command_error("close_sidecar_webview", error))
}

#[tauri::command]
pub(crate) fn scroll_sidecar_webview_to_top(
    app: AppHandle,
    sidecar_id: String,
) -> Result<(), AppError> {
    desktop::scroll_sidecar_webview_to_top_impl(app, sidecar_id)
        .map_err(|error| desktop::command_error("scroll_sidecar_webview_to_top", error))
}

#[tauri::command]
pub(crate) fn inject_sidecar_user_style(
    app: AppHandle,
    sidecar_id: String,
    user_style: String,
) -> Result<(), AppError> {
    desktop::inject_sidecar_user_style_impl(app, sidecar_id, user_style)
        .map_err(|error| desktop::command_error("inject_sidecar_user_style", error))
}
