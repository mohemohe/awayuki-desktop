// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::application::preferences;
use crate::application::settings::SettingsSnapshot;
use crate::application::translation::{self, TranslateStatusResponse};
use crate::ipc::dto::{SaveColumnsRequest, SaveSettingsRequest, TranslateStatusRequest};
use crate::ipc::error::AppError;
use crate::observability::OperationContext;
use tauri::ipc::Request as IpcRequest;
use tauri::State;

#[tauri::command]
pub(crate) async fn save_settings(
    state: State<'_, RuntimeState>,
    request: SaveSettingsRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<SettingsSnapshot, AppError> {
    desktop::observe_string_command(
        "save_settings",
        &ipc_request,
        preferences::save_settings(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn translate_status_text(
    request: TranslateStatusRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<TranslateStatusResponse, AppError> {
    desktop::observe_string_command(
        "translate_status_text",
        &ipc_request,
        translation::translate_status_text(request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn save_columns(
    state: State<'_, RuntimeState>,
    request: SaveColumnsRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<AppSnapshot, AppError> {
    let requested_id = desktop::ipc_operation_id(&ipc_request);
    let mut operation = OperationContext::start("save_columns", requested_id.as_deref(), None);
    match preferences::save_columns(state, request).await {
        Ok(snapshot) => {
            operation.finish_ok();
            Ok(snapshot)
        }
        Err(error) => {
            let app_error = preferences::save_columns_app_error(error, operation.id());
            Err(operation.finish_app_error(app_error))
        }
    }
}
