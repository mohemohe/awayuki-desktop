// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop::{self, RuntimeState};
use crate::application::media;
use crate::ipc::dto::{CancelMediaDownloadRequest, DownloadMediaRequest};
use crate::ipc::error::AppError;
use tauri::ipc::Request as IpcRequest;
use tauri::State;

#[tauri::command]
pub(crate) async fn open_status_url(
    url: String,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command("open_status_url", &ipc_request, media::open_status_url(url))
        .await
}

#[tauri::command]
pub(crate) async fn download_media(
    state: State<'_, RuntimeState>,
    request: DownloadMediaRequest,
) -> Result<(), AppError> {
    media::download_media(state.inner(), request).await
}

#[tauri::command]
pub(crate) async fn cancel_media_download(
    state: State<'_, RuntimeState>,
    request: CancelMediaDownloadRequest,
) -> Result<bool, AppError> {
    media::cancel_media_download(state.inner(), request).await
}

#[tauri::command]
pub(crate) async fn open_log_file(ipc_request: IpcRequest<'_>) -> Result<(), AppError> {
    desktop::observe_string_command("open_log_file", &ipc_request, media::open_log_file()).await
}
