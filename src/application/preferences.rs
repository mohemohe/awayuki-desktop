//! Settings mutations and their runtime effects.
//!
//! Values are committed to the portable SQLite database before logging or
//! stream lifecycle effects run. No setting is written to an OS store.

use std::future::Future;

use tauri::State;

use crate::application::desktop::{
    app_snapshot_for_state, encode_column_param_with_display_filter,
    normalized_column_account_acct, normalized_column_request, restart_streaming, AppSnapshot,
    RuntimeState,
};
use crate::application::settings as settings_application;
use crate::application::settings::SettingsSnapshot;
use crate::db::models::DbColumnConfig;
use crate::db::queries::settings;
use crate::ipc::dto::{SaveColumnsRequest, SaveSettingsRequest};
use crate::services::timeline_service::TimelineType;
use crate::state::debug_settings::DebugSettings;
use crate::state::logging;

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
) -> Result<AppSnapshot, String> {
    let columns = normalized_column_request(request.columns);
    for column in &columns {
        if TimelineType::from_column_config(&column.column_type, column.column_param.as_deref())
            .is_none()
        {
            return Err(format!("Unsupported timeline type: {}", column.column_type));
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
        });
        if configs[..index].iter().any(|config| config.id == column.id) {
            return Err(format!("Duplicate column id: {}", column.id));
        }
    }
    settings::replace_all_column_configs(state.database().writer(), &configs)
        .await
        .map_err(|error| format!("Failed to save columns atomically: {error}"))?;
    restart_streaming(state.inner()).await;
    app_snapshot_for_state(&state).await
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
}
