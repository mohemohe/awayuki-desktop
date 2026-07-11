// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use tauri::State;

#[tauri::command]
pub(crate) async fn save_settings(
    state: State<'_, RuntimeState>,
    request: SaveSettingsRequest,
) -> Result<SettingsSnapshot, AppError> {
    desktop::save_settings_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("save_settings", error))
}

#[tauri::command]
pub(crate) async fn translate_status_text(
    request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, AppError> {
    desktop::translate_status_text_command_impl(request)
        .await
        .map_err(|error| desktop::command_error("translate_status_text", error))
}

#[tauri::command]
pub(crate) async fn save_columns(
    state: State<'_, RuntimeState>,
    request: SaveColumnsRequest,
) -> Result<AppSnapshot, AppError> {
    desktop::save_columns_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("save_columns", error))
}
