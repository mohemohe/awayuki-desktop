//! Thin Tauri handlers for the JavaScript plugin runtime.

use crate::application::desktop::{self, RuntimeState};
use crate::ipc::dto::{PluginComposeButtonRequest, PluginIdRequest};
use crate::ipc::error::AppError;
use crate::plugins::PluginSnapshot;
use serde_json::Value;
use tauri::ipc::Request as IpcRequest;
use tauri::State;

async fn run_plugin_call<T, F>(call: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(call)
        .await
        .map_err(|error| format!("plugin IPC worker task failed: {error}"))?
}

#[tauri::command]
pub(crate) async fn plugin_snapshot(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<PluginSnapshot, AppError> {
    let plugins = state.plugins().clone();
    desktop::observe_string_command(
        "plugin_snapshot",
        &ipc_request,
        run_plugin_call(move || plugins.snapshot()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn open_plugin_directory(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    let directory = state.plugins().directory().to_path_buf();
    desktop::observe_string_command(
        "open_plugin_directory",
        &ipc_request,
        run_plugin_call(move || {
            std::fs::create_dir_all(&directory).map_err(|error| {
                format!(
                    "failed to create plugin directory `{}`: {error}",
                    directory.display()
                )
            })?;
            open::that(&directory).map_err(|error| error.to_string())
        }),
    )
    .await
}

#[tauri::command]
pub(crate) async fn reload_plugins(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<PluginSnapshot, AppError> {
    let plugins = state.plugins().clone();
    desktop::observe_string_command(
        "reload_plugins",
        &ipc_request,
        run_plugin_call(move || plugins.reload_all()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn reload_plugin(
    state: State<'_, RuntimeState>,
    request: PluginIdRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<PluginSnapshot, AppError> {
    let plugins = state.plugins().clone();
    desktop::observe_string_command(
        "reload_plugin",
        &ipc_request,
        run_plugin_call(move || plugins.reload_plugin(&request.plugin_id)),
    )
    .await
}

#[tauri::command]
pub(crate) async fn unload_plugin(
    state: State<'_, RuntimeState>,
    request: PluginIdRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<PluginSnapshot, AppError> {
    let plugins = state.plugins().clone();
    desktop::observe_string_command(
        "unload_plugin",
        &ipc_request,
        run_plugin_call(move || plugins.unload_plugin(&request.plugin_id)),
    )
    .await
}

#[tauri::command]
pub(crate) async fn invoke_plugin_compose_button(
    state: State<'_, RuntimeState>,
    request: PluginComposeButtonRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<Value, AppError> {
    let plugins = state.plugins().clone();
    desktop::observe_string_command(
        "invoke_plugin_compose_button",
        &ipc_request,
        run_plugin_call(move || {
            plugins.invoke_compose_button(
                &request.plugin_id,
                &request.button_id,
                request.generation,
                request.compose,
            )
        }),
    )
    .await
}
