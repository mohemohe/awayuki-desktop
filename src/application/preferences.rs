//! Settings mutations and their runtime effects.
//!
//! Values are committed to the portable SQLite database before logging or
//! stream lifecycle effects run. No setting is written to an OS store.

use std::future::Future;

use tauri::State;

use crate::api::kind::ServerKind;
use crate::application::desktop::{
    app_snapshot_for_state, encode_column_param_with_display_filter,
    normalized_column_account_acct, normalized_column_request, restart_streaming, session_for_acct,
    AppSnapshot, RuntimeState,
};
use crate::application::settings as settings_application;
use crate::application::settings::SettingsSnapshot;
use crate::db::models::DbColumnConfig;
use crate::db::queries::settings;
use crate::ipc::dto::{SaveColumnsRequest, SaveSettingsRequest};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::services::kq_filter::{
    compile_query as compile_kq_query, CompileError as KqCompileError,
};
use crate::services::timeline_service::TimelineType;
use crate::state::debug_settings::DebugSettings;
use crate::state::logging;
use crate::state::notifications::NotificationSound;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SaveColumnsError {
    #[error(transparent)]
    KqCompile(#[from] KqCompileError),
    #[error("{0}")]
    Other(String),
}

impl From<String> for SaveColumnsError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

fn validate_timeline_column(
    column_type: &str,
    column_param: Option<&str>,
) -> Result<(), SaveColumnsError> {
    let Some(timeline_type) = TimelineType::from_column_config(column_type, column_param) else {
        return Err(SaveColumnsError::Other(format!(
            "Unsupported timeline type: {column_type}"
        )));
    };
    if let TimelineType::KrileQuery(query) = timeline_type {
        compile_kq_query(&query)?;
    }
    Ok(())
}

pub(crate) fn save_columns_app_error(error: SaveColumnsError, request_id: &str) -> AppError {
    match error {
        SaveColumnsError::KqCompile(error) => {
            let line = error.line();
            let column = error.column();
            AppError::from_code(
                AppErrorCode::Validation,
                "KQ query failed static validation",
                request_id,
            )
            .with_message_key("errors.kq_invalid_query")
            .with_safe_detail("line", line)
            .with_safe_detail("column", column)
        }
        SaveColumnsError::Other(error) => AppError::from_source(error, request_id),
    }
}

async fn commit_then_apply<T, E, Commit, CommitFuture, Apply, ApplyFuture>(
    commit: Commit,
    apply: Apply,
) -> Result<T, E>
where
    Commit: FnOnce() -> CommitFuture,
    CommitFuture: Future<Output = Result<(), E>>,
    Apply: FnOnce() -> ApplyFuture,
    ApplyFuture: Future<Output = Result<T, E>>,
{
    commit().await?;
    apply().await
}

pub(crate) async fn save_settings(
    state: State<'_, RuntimeState>,
    request: SaveSettingsRequest,
) -> Result<SettingsSnapshot, String> {
    let json = settings_application::validated_settings_json(&request.key, request.value)?;
    let key = request.key;
    commit_then_apply(
        || async {
            settings::set_setting(state.database().writer(), &key, &json)
                .await
                .map_err(|error| error.to_string())
        },
        || async {
            if key == "debug" {
                let debug = serde_json::from_str::<DebugSettings>(&json).map_err(|error| {
                    format!("Validated debug settings could not be read: {error}")
                })?;
                if debug.logging_enabled {
                    logging::enable().map_err(|error| error.to_string())?;
                } else {
                    logging::disable();
                }
                logging::set_log_level(debug.log_level);
            }
            if key == "bluesky_fetch" {
                restart_streaming(state.inner()).await;
            }
            settings_application::settings_snapshot(state.database()).await
        },
    )
    .await
}

pub(crate) async fn save_columns(
    state: State<'_, RuntimeState>,
    request: SaveColumnsRequest,
) -> Result<AppSnapshot, SaveColumnsError> {
    let columns = normalized_column_request(request.columns);
    for column in &columns {
        // Validate before constructing the replacement set so an invalid KQ
        // can never reach the atomic persistence call below.
        validate_timeline_column(&column.column_type, column.column_param.as_deref())?;
        if column.column_type == "feed" {
            let acct = column
                .account_acct
                .as_deref()
                .map(str::trim)
                .filter(|acct| !acct.is_empty())
                .ok_or_else(|| {
                    SaveColumnsError::Other(
                        "Feed timeline requires a Bluesky source account".to_string(),
                    )
                })?;
            let session = session_for_acct(state.inner(), acct).await.ok_or_else(|| {
                SaveColumnsError::Other(format!("Account is not signed in: {acct}"))
            })?;
            if session.client.kind() != ServerKind::Bluesky {
                return Err(SaveColumnsError::Other(
                    "Feed timeline requires a Bluesky source account".to_string(),
                ));
            }
        }
        if let Some(sound) = column.notification_sound.as_deref() {
            if NotificationSound::parse(sound).is_none() {
                return Err(SaveColumnsError::Other(format!(
                    "Unsupported notification sound: {sound}"
                )));
            }
        }
    }

    let mut configs = Vec::with_capacity(columns.len());
    for (index, column) in columns.into_iter().enumerate() {
        let account_acct = normalized_column_account_acct(&column)?;
        configs.push(DbColumnConfig {
            id: column.id.clone(),
            account_acct,
            column_type: column.column_type.clone(),
            column_param: encode_column_param_with_display_filter(&column),
            position: column.position,
            width: None,
            name: Some(column.name.clone()),
            max_statuses: Some(column.max_statuses.max(1) as i32),
            pane_index: Some(column.pane_index as i32),
            desktop_notifications: column.desktop_notifications.unwrap_or(true),
            notification_sound: column.notification_sound.clone(),
        });
        if configs[..index].iter().any(|config| config.id == column.id) {
            return Err(SaveColumnsError::Other(format!(
                "Duplicate column id: {}",
                column.id
            )));
        }
    }
    settings::replace_all_column_configs(state.database().writer(), &configs)
        .await
        .map_err(|error| {
            SaveColumnsError::Other(format!("Failed to save columns atomically: {error}"))
        })?;
    restart_streaming(state.inner()).await;
    app_snapshot_for_state(&state)
        .await
        .map_err(SaveColumnsError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn runtime_effect_starts_only_after_sqlite_commit() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let committed = events.clone();
        let applied = events.clone();
        let result = commit_then_apply(
            move || async move {
                committed.lock().unwrap().push("sqlite-commit");
                Ok::<_, String>(())
            },
            move || async move {
                applied.lock().unwrap().push("runtime-effect");
                Ok::<_, String>(())
            },
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(
            events.lock().unwrap().as_slice(),
            ["sqlite-commit", "runtime-effect"]
        );
    }

    #[tokio::test]
    async fn failed_sqlite_commit_skips_runtime_effect() {
        let applied = Arc::new(Mutex::new(false));
        let applied_callback = applied.clone();
        let result = commit_then_apply(
            || async { Err::<(), _>("commit failed") },
            move || async move {
                *applied_callback.lock().unwrap() = true;
                Ok::<_, &str>(())
            },
        )
        .await;
        assert_eq!(result, Err("commit failed"));
        assert!(!*applied.lock().unwrap());
    }

    #[test]
    fn invalid_kq_is_rejected_before_persistence_with_safe_source_position() {
        const SECRET_SHAPED_UNKNOWN_SOURCE: &str = "from private_search__bearer_sk_live_7fd93ac1";
        let compile_error = match validate_timeline_column("kq", Some(SECRET_SHAPED_UNKNOWN_SOURCE))
        {
            Err(SaveColumnsError::KqCompile(error)) => error,
            other => panic!("unknown KQ source must fail static validation, got {other:?}"),
        };
        let error = save_columns_app_error(
            SaveColumnsError::KqCompile(compile_error),
            "request-save-kq",
        );

        assert_eq!(error.code, AppErrorCode::Validation);
        assert!(!error.retryable);
        assert_eq!(error.message_key, "errors.kq_invalid_query");
        assert!(error.safe_details.contains_key("line"));
        assert!(error.safe_details.contains_key("column"));
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("bearer_sk_live_7fd93ac1"));
    }

    #[test]
    fn valid_kq_passes_the_pre_persistence_validation_gate() {
        assert!(
            validate_timeline_column("kq", Some("from home where text contains \"Awayuki\""))
                .is_ok()
        );
    }
}
