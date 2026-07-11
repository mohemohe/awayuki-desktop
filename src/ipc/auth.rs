// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use tauri::State;

#[tauri::command]
pub(crate) async fn login_with_instance_domain(
    state: State<'_, RuntimeState>,
    request: LoginInstanceRequest,
) -> Result<AppSnapshot, AppError> {
    desktop::login_with_instance_domain_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("login_with_instance_domain", error))
}

#[tauri::command]
pub(crate) async fn login_with_bluesky_app_password(
    state: State<'_, RuntimeState>,
    request: LoginBlueskyRequest,
) -> Result<AppSnapshot, AppError> {
    desktop::login_with_bluesky_app_password_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("login_with_bluesky_app_password", error))
}
