//! Frontend-ready runtime initialization coordination.
//!
//! The WebView calls this boundary only after progress listeners exist. Heavy
//! portable SQLite migration and session restoration run on this module's
//! background worker, never in synchronous Tauri setup.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use tauri::{AppHandle, Manager, State};

use crate::api::kind::ServerKind;
use crate::application::desktop::{
    app_snapshot_for_state, install_window_state_persistence, restart_streaming, restore_session,
    restore_window_state, schedule_post_ready_work, AppSnapshot, AppStartupProgressEvent,
    RuntimeState, APP_STARTUP_PROGRESS_EVENT,
};
use crate::application::settings as settings_application;
use crate::application::startup_gate::RetryStartError;
use crate::auth::credential_store::AccountCredentials;
use crate::auth::session::SessionManager;
use crate::db::queries::settings;
use crate::ipc::dto::CancelMutationOperationRequest;
use crate::ipc::error::AppError;
use crate::ipc::error::AppErrorCode;
use crate::observability::OperationContext;
use crate::state::debug_settings::DebugSettings;
use crate::state::logging;

pub(crate) async fn app_snapshot(state: State<'_, RuntimeState>) -> Result<AppSnapshot, String> {
    state.startup_gate().wait_until_ready().await?;
    app_snapshot_for_state(&state).await
}

pub(crate) async fn cancel_mutation_operation(
    state: State<'_, RuntimeState>,
    request: CancelMutationOperationRequest,
) -> Result<bool, AppError> {
    let mut operation = OperationContext::start(
        "cancel_mutation_operation",
        request.operation_id.as_deref(),
        None,
    );
    if uuid::Uuid::parse_str(&request.target_operation_id).is_err() {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "target operation ID must be a UUID",
        ));
    }
    let cancelled = state
        .mutation_operation_manager()
        .cancel(&request.target_operation_id);
    operation.finish_ok();
    Ok(cancelled)
}

pub(crate) async fn start_runtime_initialization(
    state: State<'_, RuntimeState>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    // Duplicate frontend handshakes coalesce into the already-running worker.
    if !state.startup_gate().begin_initialization() {
        tracing::warn!("Skipped duplicate initial runtime worker");
        return Ok(());
    }
    spawn_runtime_initialization_worker(state.inner().clone(), app, operation_id);
    Ok(())
}

pub(crate) async fn retry_runtime_initialization(
    state: State<'_, RuntimeState>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<(), AppError> {
    let mut operation = OperationContext::start(
        "retry_runtime_initialization",
        operation_id.as_deref(),
        None,
    );
    match state.startup_gate().begin_retry() {
        Ok(()) => {
            spawn_runtime_initialization_worker(state.inner().clone(), app, operation_id);
            operation.finish_ok();
            Ok(())
        }
        Err(RetryStartError::AlreadyRunning) => {
            operation.finish_ok();
            Ok(())
        }
        Err(error @ RetryStartError::NotFailed) => {
            let app_error = AppError::validation(operation.id());
            tracing::warn!(error = %error, "Rejected runtime retry in non-failed state");
            Err(operation.finish_app_error(app_error))
        }
    }
}

fn spawn_runtime_initialization_worker(
    state: RuntimeState,
    app_handle: AppHandle,
    operation_id: Option<String>,
) {
    // SQLx migration futures can be !Send. Run them on a dedicated blocking
    // worker while the WebView/main thread remains available for progress UI.
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(async move {
            let mut operation = OperationContext::start("startup", operation_id.as_deref(), None);
            let failure_stage = AtomicU8::new(StartupFailureStage::Database as u8);
            match initialize_runtime_state(&state, &app_handle, &operation, &failure_stage).await {
                Ok(()) => operation.finish_ok(),
                Err(error) => {
                    tracing::error!(error = %error, "Awayuki background initialization failed");
                    let stage = StartupFailureStage::from_u8(failure_stage.load(Ordering::Acquire));
                    let public_message = stage.public_message();
                    state.startup_gate().mark_failed(public_message);
                    emit_app_startup_progress(
                        &state,
                        stage.as_str(),
                        "error",
                        Some(public_message.to_string()),
                    )
                    .await;
                    let _ = operation.finish_error(error);
                }
            }
        });
    });
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum StartupFailureStage {
    Database = 0,
    Settings = 1,
    Sessions = 2,
    Services = 3,
}

impl StartupFailureStage {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Settings,
            2 => Self::Sessions,
            3 => Self::Services,
            _ => Self::Database,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Settings => "settings",
            Self::Sessions => "sessions",
            Self::Services => "services",
        }
    }

    fn public_message(self) -> &'static str {
        match self {
            Self::Database => "The portable database could not be initialized",
            Self::Settings => "Application settings could not be restored",
            Self::Sessions => "Account sessions could not be restored",
            Self::Services => "Background services could not be started",
        }
    }
}

async fn initialize_runtime_state(
    state: &RuntimeState,
    app_handle: &AppHandle,
    startup_operation: &OperationContext,
    failure_stage: &AtomicU8,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    failure_stage.store(StartupFailureStage::Database as u8, Ordering::Release);
    startup_operation.phase("db");
    emit_app_startup_progress(
        state,
        "database",
        "running",
        Some("Preparing the portable database".to_string()),
    )
    .await;

    let migration_started = Instant::now();
    let migration_report = state.database().run_migrations().await?;
    tracing::info!(
        repaired_legacy_schema = migration_report.repaired_legacy_schema,
        applied_versions = ?migration_report.applied_versions,
        duration_ms = elapsed_ms(migration_started),
        "Database migration check completed"
    );
    emit_app_startup_progress(
        state,
        "database",
        "complete",
        Some("Portable database ready".to_string()),
    )
    .await;

    failure_stage.store(StartupFailureStage::Settings as u8, Ordering::Release);
    emit_app_startup_progress(
        state,
        "settings",
        "running",
        Some("Restoring application settings".to_string()),
    )
    .await;
    if let Some(previous) = settings::ensure_settings_schema_version(
        state.database().writer(),
        crate::ipc::contract::SETTINGS_SCHEMA_VERSION,
    )
    .await?
    {
        tracing::info!(
            previous,
            current = crate::ipc::contract::SETTINGS_SCHEMA_VERSION,
            "Settings schema contract recorded in portable SQLite database"
        );
    }
    apply_debug_logging_settings(state).await;
    emit_app_startup_progress(
        state,
        "settings",
        "complete",
        Some("Application settings restored".to_string()),
    )
    .await;

    startup_operation.phase("api");
    failure_stage.store(StartupFailureStage::Sessions as u8, Ordering::Release);
    emit_app_startup_progress(
        state,
        "sessions",
        "running",
        Some("Restoring account sessions".to_string()),
    )
    .await;
    let mut sessions = SessionManager::new();
    let accounts = settings::get_login_accounts(state.database().reader()).await?;
    let active_acct = accounts
        .iter()
        .find(|account| account.is_active)
        .map(|account| account.acct.clone());

    for account in accounts {
        let account_credentials = AccountCredentials::from_login_account(&account);
        match restore_session(&account, &account_credentials).await {
            Ok(session) => {
                if matches!(session.client.kind(), ServerKind::Bluesky) {
                    let access_token = session.client.current_access_token().await?;
                    let app_password = session.client.bluesky_app_password();
                    state
                        .credentials()
                        .update_for_account(
                            state.database().writer(),
                            &session.acct,
                            &AccountCredentials::new(access_token, app_password),
                        )
                        .await?;
                    session.client.set_bluesky_credential_sink(
                        state
                            .credentials()
                            .bluesky_sink(state.database().writer(), session.acct.clone()),
                    );
                }
                sessions.add_session(session)
            }
            Err(error) => tracing::warn!("Failed to restore session {}: {}", account.acct, error),
        }
    }

    if let Some(acct) = active_acct {
        sessions.set_active(&acct);
    }
    *state.sessions().write().await = sessions;
    emit_app_startup_progress(
        state,
        "sessions",
        "complete",
        Some("Account sessions restored".to_string()),
    )
    .await;

    emit_app_startup_progress(
        state,
        "services",
        "running",
        Some("Starting local services".to_string()),
    )
    .await;
    failure_stage.store(StartupFailureStage::Services as u8, Ordering::Release);
    if let Some(window) = app_handle.get_webview_window("main") {
        restore_window_state(&window, state.database()).await;
        install_window_state_persistence(window, state.database_handle());
    }
    // Rebuild streams from every restored session. Active account remains only
    // the mutation actor and does not select a Home/Public/Notification source.
    restart_streaming(state).await;

    state.startup_gate().mark_ready();
    emit_app_startup_progress(
        state,
        "ready",
        "complete",
        Some("Awayuki is ready".to_string()),
    )
    .await;
    schedule_post_ready_work(state);
    Ok(())
}

async fn emit_app_startup_progress(
    state: &RuntimeState,
    stage: &'static str,
    status: &'static str,
    message: Option<String>,
) {
    state
        .emit_application_event(
            APP_STARTUP_PROGRESS_EVENT,
            AppStartupProgressEvent {
                stage,
                status,
                message,
            },
            "application startup status",
        )
        .await;
}

async fn apply_debug_logging_settings(state: &RuntimeState) {
    let debug = match settings_application::load_setting::<DebugSettings>(state.database(), "debug")
        .await
    {
        Ok(debug) => debug,
        Err(error) => {
            tracing::warn!("Failed to load debug settings for logging: {}", error);
            DebugSettings::default()
        }
    };
    if debug.logging_enabled {
        if let Err(error) = logging::enable() {
            tracing::warn!("Failed to enable file logging: {}", error);
        }
    }
    logging::set_log_level(debug.log_level);
}

fn elapsed_ms(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}
