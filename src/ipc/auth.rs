// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::auth;
use crate::application::desktop::{AppSnapshot, RuntimeState};
use crate::ipc::dto::{CancelLoginFlowRequest, LoginBlueskyRequest, LoginInstanceRequest};
use crate::ipc::error::AppError;
use tauri::State;

#[tauri::command]
pub(crate) async fn login_with_instance_domain(
    state: State<'_, RuntimeState>,
    request: LoginInstanceRequest,
) -> Result<AppSnapshot, AppError> {
    auth::login_with_instance_domain(state.inner(), request).await
}

#[tauri::command]
pub(crate) async fn login_with_bluesky_app_password(
    state: State<'_, RuntimeState>,
    request: LoginBlueskyRequest,
) -> Result<AppSnapshot, AppError> {
    auth::login_with_bluesky_app_password(state.inner(), request).await
}

#[tauri::command]
pub(crate) async fn cancel_login_flow(
    state: State<'_, RuntimeState>,
    request: CancelLoginFlowRequest,
) -> Result<bool, AppError> {
    auth::cancel_login_flow(state.inner(), request).await
}
