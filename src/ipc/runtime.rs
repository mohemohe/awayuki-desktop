//! Desktop runtime entry point and startup handshake commands.

use crate::application::desktop;
use crate::application::desktop::{AppSnapshot, RuntimeState};
use crate::application::runtime;
use crate::ipc::dto::{CancelMutationOperationRequest, ReleaseWebviewSmokeReport};
use crate::ipc::error::AppError;
use tauri::ipc::Request as IpcRequest;
use tauri::{AppHandle, State};

#[tauri::command]
pub(crate) async fn app_snapshot(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<AppSnapshot, AppError> {
    desktop::observe_string_command("app_snapshot", &ipc_request, runtime::app_snapshot(state))
        .await
}

#[tauri::command]
pub(crate) async fn cancel_mutation_operation(
    state: State<'_, RuntimeState>,
    request: CancelMutationOperationRequest,
) -> Result<bool, AppError> {
    runtime::cancel_mutation_operation(state, request).await
}

#[tauri::command]
pub(crate) fn report_release_webview_smoke(
    report: ReleaseWebviewSmokeReport,
) -> Result<(), AppError> {
    let enabled = std::env::var_os("AWAYUKI_RELEASE_SECURITY_SMOKE").is_some()
        && std::env::var_os("AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL").is_some();
    let passed = release_webview_smoke_passed(&report);
    if !enabled || !passed {
        return Err(AppError::validation("release-webview-smoke"));
    }
    let payload = serde_json::to_string(&report)
        .map_err(|_| AppError::validation("release-webview-smoke"))?;
    println!("AWAYUKI_WEBVIEW_SECURITY_REPORT {payload}");
    Ok(())
}

fn release_webview_smoke_passed(report: &ReleaseWebviewSmokeReport) -> bool {
    report.image_loaded
        && report.protocol_media_loaded
        && report.custom_emoji_loaded
        && report.video_loaded
        && report.sidecar_created
        && report.sidecar_hidden_during_preview
        && report.sidecar_restored
        && report.sidecar_closed
        && report.csp_violation_count == 0
}

/// Frontend-ready handshake: migrations cannot begin before this command.
#[tauri::command]
pub(crate) async fn start_runtime_initialization(
    state: State<'_, RuntimeState>,
    app: AppHandle,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    let operation_id = desktop::ipc_operation_id(&ipc_request);
    runtime::start_runtime_initialization(state, app, operation_id).await
}

#[tauri::command]
pub(crate) async fn retry_runtime_initialization(
    state: State<'_, RuntimeState>,
    app: AppHandle,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    let operation_id = desktop::ipc_operation_id(&ipc_request);
    runtime::retry_runtime_initialization(state, app, operation_id).await
}

pub fn run() {
    crate::application::desktop::run();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed_report() -> ReleaseWebviewSmokeReport {
        ReleaseWebviewSmokeReport {
            image_loaded: true,
            protocol_media_loaded: true,
            custom_emoji_loaded: true,
            video_loaded: true,
            sidecar_created: true,
            sidecar_hidden_during_preview: true,
            sidecar_restored: true,
            sidecar_closed: true,
            csp_violation_count: 0,
        }
    }

    #[test]
    fn release_webview_smoke_requires_every_operation_and_a_clean_csp_report() {
        let report = passed_report();
        assert!(release_webview_smoke_passed(&report));

        let mut violation = report.clone();
        violation.csp_violation_count = 1;
        assert!(!release_webview_smoke_passed(&violation));

        let mut hidden = report;
        hidden.sidecar_hidden_during_preview = false;
        assert!(!release_webview_smoke_passed(&hidden));
    }
}
