//! Desktop runtime entry point and startup handshake commands.

use crate::application::desktop;
use crate::application::desktop::{AppSnapshot, RuntimeState};
use crate::ipc::error::AppError;
use tauri::{AppHandle, State};

#[tauri::command]
pub(crate) async fn app_snapshot(state: State<'_, RuntimeState>) -> Result<AppSnapshot, AppError> {
    desktop::app_snapshot_impl(state)
        .await
        .map_err(|error| desktop::command_error("app_snapshot", error))
}

/// Frontend-ready handshake: migrations cannot begin before this command.
#[tauri::command]
pub(crate) async fn start_runtime_initialization(
    state: State<'_, RuntimeState>,
    app: AppHandle,
) -> Result<(), AppError> {
    desktop::start_runtime_initialization_impl(state, app).await
}

#[tauri::command]
pub(crate) async fn retry_runtime_initialization(
    state: State<'_, RuntimeState>,
    app: AppHandle,
) -> Result<(), AppError> {
    desktop::retry_runtime_initialization_impl(state, app).await
}

pub fn run() {
    crate::application::desktop::run();
}
