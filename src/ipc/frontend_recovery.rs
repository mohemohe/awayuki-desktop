//! Main-WebView-only access to the in-memory recovery checkpoint.

use serde_json::Value;
use tauri::{State, Webview};

use crate::application::frontend_recovery::{
    CheckpointError, FrontendRecoveryState, MAX_CHECKPOINT_BYTES,
};
use crate::ipc::dto::{FrontendRecoveryCheckpointRequest, FrontendRecoveryRestoreResponse};
use crate::ipc::error::{AppError, AppErrorCode};

#[tauri::command]
pub(crate) fn checkpoint_frontend_recovery(
    webview: Webview,
    state: State<'_, FrontendRecoveryState>,
    request: FrontendRecoveryCheckpointRequest,
) -> Result<(), AppError> {
    ensure_main_webview(webview.label())?;
    state
        .checkpoint(
            request.checkpoint,
            request.recovery_blocked,
            &request.generation,
        )
        .map_err(checkpoint_error)
}

#[tauri::command]
pub(crate) fn restore_frontend_recovery(
    webview: Webview,
    state: State<'_, FrontendRecoveryState>,
) -> Result<FrontendRecoveryRestoreResponse, AppError> {
    ensure_main_webview(webview.label())?;
    let restored = state.restore().map_err(checkpoint_error)?;
    Ok(FrontendRecoveryRestoreResponse {
        checkpoint: restored.checkpoint.unwrap_or(Value::Null),
        generation: restored.generation,
    })
}

fn ensure_main_webview(label: &str) -> Result<(), AppError> {
    if label == "main" {
        Ok(())
    } else {
        Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            "frontend-recovery",
        ))
    }
}

fn checkpoint_error(error: CheckpointError) -> AppError {
    match error {
        CheckpointError::TooLarge => AppError::validation("frontend-recovery")
            .with_safe_detail("field", "checkpoint")
            .with_safe_detail("limit", MAX_CHECKPOINT_BYTES),
        CheckpointError::Unavailable => AppError::new(AppErrorCode::Internal, "frontend-recovery"),
        CheckpointError::StaleDocument => {
            AppError::new(AppErrorCode::Cancelled, "frontend-recovery")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_webviews_cannot_read_or_replace_the_main_draft() {
        assert!(ensure_main_webview("main").is_ok());
        for label in ["sidecar-x", "main-child", "", "settings"] {
            assert_eq!(
                ensure_main_webview(label).unwrap_err().code,
                AppErrorCode::CapabilityUnsupported
            );
        }
    }

    #[test]
    fn stale_document_rejection_cannot_be_retried_and_exposes_no_checkpoint_details() {
        let error = checkpoint_error(CheckpointError::StaleDocument);
        assert_eq!(error.code, AppErrorCode::Cancelled);
        assert!(!error.retryable);
        assert!(error.safe_details.is_empty());
    }

    #[test]
    fn checkpoint_requires_a_document_generation() {
        let request = serde_json::json!({ "checkpoint": null, "recoveryBlocked": false });
        assert!(serde_json::from_value::<FrontendRecoveryCheckpointRequest>(request).is_err());
    }
}
