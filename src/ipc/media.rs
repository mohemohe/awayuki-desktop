// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;

#[tauri::command]
pub(crate) async fn open_status_url(url: String) -> Result<(), AppError> {
    desktop::open_status_url_impl(url)
        .await
        .map_err(|error| desktop::command_error("open_status_url", error))
}

#[tauri::command]
pub(crate) async fn download_media(request: DownloadMediaRequest) -> Result<(), AppError> {
    desktop::download_media_impl(request)
        .await
        .map_err(|error| desktop::command_error("download_media", error))
}

#[tauri::command]
pub(crate) async fn open_log_file() -> Result<(), AppError> {
    desktop::open_log_file_impl()
        .await
        .map_err(|error| desktop::command_error("open_log_file", error))
}
