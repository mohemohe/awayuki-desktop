use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::api::kind::ServerKind;
use crate::application::desktop::acting_session;
use crate::application::desktop::{RuntimeState, TimelineStatus};
use crate::application::status;
use crate::db::queries::compose_outbox::{self, ComposeOutboxRow};
use crate::ipc::dto::{EditStatusRequest, PostRequest};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::observability::OperationContext;

pub(crate) const COMPOSE_OUTBOX_UPDATED_EVENT: &str = "compose-outbox-updated";
const MAX_AUTOMATIC_ATTEMPTS: i64 = 6;
const MAX_CONCURRENT_DELIVERIES: usize = 4;
const ENQUEUE_WRITER_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComposeOutboxItemView {
    pub(crate) id: String,
    pub(crate) operation_kind: String,
    pub(crate) acting_account_acct: String,
    pub(crate) content_preview: String,
    pub(crate) state: String,
    pub(crate) attempts: i64,
    pub(crate) last_error: Option<String>,
    pub(crate) next_attempt_at: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) completed_at: Option<String>,
    pub(crate) result_status_id: Option<String>,
    pub(crate) result_server_domain: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ComposeOutboxUpdatedPayload {
    item: ComposeOutboxItemView,
    status: Option<TimelineStatus>,
}

pub(crate) async fn enqueue_post(
    state: &RuntimeState,
    mut request: PostRequest,
    operation_id: String,
) -> Result<ComposeOutboxItemView, AppError> {
    request.operation_id = Some(operation_id.clone());
    enqueue(
        state,
        &operation_id,
        "post",
        &request.acting_account_acct,
        &request,
    )
    .await
}

pub(crate) async fn enqueue_edit(
    state: &RuntimeState,
    request: EditStatusRequest,
    operation_id: String,
) -> Result<ComposeOutboxItemView, AppError> {
    enqueue(
        state,
        &operation_id,
        "edit",
        &request.acting_account_acct,
        &request,
    )
    .await
}

async fn enqueue<T: serde::Serialize>(
    state: &RuntimeState,
    operation_id: &str,
    operation_kind: &str,
    acting_account_acct: &str,
    request: &T,
) -> Result<ComposeOutboxItemView, AppError> {
    let command = match operation_kind {
        "edit" => "enqueue_edit_status",
        _ => "enqueue_post_status",
    };
    let mut operation =
        OperationContext::start(command, Some(operation_id), Some(acting_account_acct));
    let payload = match serde_json::to_string(request) {
        Ok(payload) => payload,
        Err(error) => {
            let error = AppError::from_source(error, operation_id);
            return Err(operation.finish_app_error(error));
        }
    };
    let now = now();
    operation.phase("writer_wait");
    let writer_wait_started_at = Instant::now();
    let mut writer = match tokio::time::timeout(
        ENQUEUE_WRITER_ACQUIRE_TIMEOUT,
        state.database().writer().acquire(),
    )
    .await
    {
        Ok(Ok(writer)) => writer,
        Ok(Err(error)) => {
            let error = AppError::from_database(error, operation_id);
            return Err(operation.finish_app_error(error));
        }
        Err(_) => {
            let error = AppError::from_database(sqlx::Error::PoolTimedOut, operation_id);
            return Err(operation.finish_app_error(error));
        }
    };
    let writer_wait_ms = writer_wait_started_at
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    tracing::debug!(
        operation_id,
        writer_wait_ms,
        "Compose outbox writer acquired"
    );
    operation.phase("db");
    let database_started_at = Instant::now();
    let row = match compose_outbox::enqueue_on(
        &mut writer,
        operation_id,
        operation_kind,
        acting_account_acct,
        &payload,
        &now,
    )
    .await
    {
        Ok(row) => {
            crate::observability::observe_db_query(
                1,
                database_started_at
                    .elapsed()
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            );
            row
        }
        Err(error) => {
            crate::observability::observe_db_query(
                0,
                database_started_at
                    .elapsed()
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            );
            let error = AppError::from_database(error, operation_id);
            return Err(operation.finish_app_error(error));
        }
    };
    let item = row_to_view(row);
    operation.phase("queued");
    emit_update(state, item.clone(), None);
    state.compose_outbox_notify().notify_one();
    operation.finish_ok();
    Ok(item)
}

pub(crate) async fn list(
    state: &RuntimeState,
    operation_id: &str,
) -> Result<Vec<ComposeOutboxItemView>, AppError> {
    compose_outbox::list(state.database().reader())
        .await
        .map(|rows| rows.into_iter().map(row_to_view).collect())
        .map_err(|error| AppError::from_database(error, operation_id))
}

pub(crate) async fn retry(
    state: &RuntimeState,
    id: &str,
    operation_id: &str,
) -> Result<ComposeOutboxItemView, AppError> {
    let item = compose_outbox::retry(state.database().writer(), id, &now())
        .await
        .map_err(|error| AppError::from_database(error, operation_id))?
        .ok_or_else(|| {
            AppError::from_code(
                AppErrorCode::Validation,
                "Only failed or cancelled outbox items can be retried",
                operation_id,
            )
        })
        .map(row_to_view)?;
    emit_update(state, item.clone(), None);
    state.compose_outbox_notify().notify_one();
    Ok(item)
}

pub(crate) async fn cancel(
    state: &RuntimeState,
    id: &str,
    operation_id: &str,
) -> Result<ComposeOutboxItemView, AppError> {
    let item = compose_outbox::cancel(state.database().writer(), id, &now())
        .await
        .map_err(|error| AppError::from_database(error, operation_id))?
        .ok_or_else(|| {
            AppError::from_code(
                AppErrorCode::Validation,
                "A sending or completed outbox item cannot be cancelled",
                operation_id,
            )
        })
        .map(row_to_view)?;
    emit_update(state, item.clone(), None);
    Ok(item)
}

pub(crate) fn schedule(
    state: RuntimeState,
    started: &std::sync::atomic::AtomicBool,
    cancellation: CancellationToken,
    notify: Arc<Notify>,
) {
    use std::sync::atomic::Ordering;
    if started.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(error) =
            compose_outbox::recover_interrupted(state.database().writer(), &now()).await
        {
            tracing::warn!(%error, "Failed to recover interrupted compose outbox items");
        }
        recover_idempotent_interrupted_items(&state).await;
        tracing::info!("Compose outbox worker started");
        let mut deliveries = tokio::task::JoinSet::new();
        loop {
            if cancellation.is_cancelled() {
                break;
            }
            let mut claimed = false;
            while deliveries.len() < MAX_CONCURRENT_DELIVERIES {
                match compose_outbox::has_due(state.database().reader(), &now()).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        tracing::warn!(%error, "Failed to inspect compose outbox queue");
                        break;
                    }
                }
                match compose_outbox::claim_next(state.database().writer(), &now()).await {
                    Ok(Some(row)) => {
                        let sending = row_to_view(row.clone());
                        emit_update(&state, sending, None);
                        let delivery_state = state.clone();
                        deliveries.spawn(async move {
                            deliver(&delivery_state, row).await;
                        });
                        claimed = true;
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "Failed to claim compose outbox item");
                        break;
                    }
                }
            }
            if claimed {
                continue;
            }
            tokio::select! {
                _ = cancellation.cancelled() => break,
                _ = notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                completed = deliveries.join_next(), if !deliveries.is_empty() => {
                    if let Some(Err(error)) = completed {
                        tracing::warn!(%error, "Compose outbox delivery task stopped unexpectedly");
                    }
                }
            }
        }
        deliveries.abort_all();
        tracing::info!("Compose outbox worker stopped");
    });
}

async fn recover_idempotent_interrupted_items(state: &RuntimeState) {
    let Ok(rows) = compose_outbox::list(state.database().reader()).await else {
        return;
    };
    for row in rows.into_iter().filter(|row| {
        row.state == "uncertain" && row.last_error.as_deref() == Some("errors.delivery_uncertain")
    }) {
        let retry_is_safe = acting_session(state, &row.acting_account_acct)
            .await
            .map(|session| {
                matches!(
                    session.client.kind(),
                    ServerKind::Mastodon | ServerKind::Paon | ServerKind::Bluesky
                )
            })
            .unwrap_or(false);
        if !retry_is_safe {
            continue;
        }
        if let Err(error) = compose_outbox::retry(state.database().writer(), &row.id, &now()).await
        {
            tracing::warn!(
                outbox_id = row.id,
                %error,
                "Failed to resume idempotent interrupted compose outbox item"
            );
        }
    }
}

async fn deliver(state: &RuntimeState, row: ComposeOutboxRow) {
    let idempotent_provider = acting_session(state, &row.acting_account_acct)
        .await
        .map(|session| {
            matches!(
                session.client.kind(),
                ServerKind::Mastodon | ServerKind::Paon | ServerKind::Bluesky
            )
        })
        .unwrap_or(false);
    let result = match row.operation_kind.as_str() {
        "post" => match serde_json::from_str::<PostRequest>(&row.payload_json) {
            Ok(request) => status::deliver_queued_post(state, request, &row.id).await,
            Err(error) => Err(AppError::from_source(error, &row.id)),
        },
        "edit" => match serde_json::from_str::<EditStatusRequest>(&row.payload_json) {
            Ok(request) => status::deliver_queued_edit(state, request, &row.id).await,
            Err(error) => Err(AppError::from_source(error, &row.id)),
        },
        _ => Err(AppError::from_code(
            AppErrorCode::Validation,
            "Unknown compose outbox operation",
            &row.id,
        )),
    };

    match result {
        Ok(status) => {
            let timestamp = now();
            match compose_outbox::mark_succeeded(
                state.database().writer(),
                &row.id,
                &status.original_status_id,
                &status.server_domain,
                &timestamp,
            )
            .await
            {
                Ok(completed) => {
                    emit_update(state, row_to_view(completed), Some(status));
                }
                Err(error) => {
                    tracing::error!(
                        outbox_id = row.id,
                        %error,
                        "Provider accepted compose outbox item but completion could not be persisted"
                    );
                }
            }
        }
        Err(error) => {
            let timestamp = now();
            let retry_is_unambiguous = matches!(
                error.code,
                AppErrorCode::RateLimited | AppErrorCode::DatabaseBusy
            );
            let delivery_may_be_ambiguous =
                matches!(error.code, AppErrorCode::Timeout | AppErrorCode::Internal);
            let should_retry = row.attempts < MAX_AUTOMATIC_ATTEMPTS
                && (retry_is_unambiguous || (idempotent_provider && delivery_may_be_ambiguous));
            let updated = if should_retry {
                let delay = retry_delay(row.attempts, &error);
                let next_attempt_at = (Utc::now() + chrono::Duration::from_std(delay).unwrap())
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
                compose_outbox::mark_retrying(
                    state.database().writer(),
                    &row.id,
                    &error.message_key,
                    &next_attempt_at,
                    &timestamp,
                )
                .await
            } else if delivery_may_be_ambiguous && !idempotent_provider {
                compose_outbox::mark_uncertain(
                    state.database().writer(),
                    &row.id,
                    "errors.delivery_uncertain",
                    &timestamp,
                )
                .await
            } else {
                compose_outbox::mark_failed(
                    state.database().writer(),
                    &row.id,
                    &error.message_key,
                    &timestamp,
                )
                .await
            };
            match updated {
                Ok(updated) => emit_update(state, row_to_view(updated), None),
                Err(db_error) => tracing::error!(
                    outbox_id = row.id,
                    %db_error,
                    "Failed to persist compose outbox delivery failure"
                ),
            }
        }
    }
}

fn retry_delay(attempts: i64, error: &AppError) -> Duration {
    if let Some(seconds) = error
        .safe_details
        .get("retryAfterSeconds")
        .and_then(|value| value.parse::<u64>().ok())
    {
        return Duration::from_secs(seconds.clamp(1, 3600));
    }
    let seconds = match attempts {
        ..=1 => 5,
        2 => 15,
        3 => 45,
        4 => 120,
        _ => 300,
    };
    Duration::from_secs(seconds)
}

fn emit_update(state: &RuntimeState, item: ComposeOutboxItemView, status: Option<TimelineStatus>) {
    state.try_emit_application_event(
        COMPOSE_OUTBOX_UPDATED_EVENT,
        ComposeOutboxUpdatedPayload { item, status },
        "compose outbox update",
    );
}

fn row_to_view(row: ComposeOutboxRow) -> ComposeOutboxItemView {
    let content = match row.operation_kind.as_str() {
        "post" => serde_json::from_str::<PostRequest>(&row.payload_json)
            .map(|request| request.status)
            .unwrap_or_default(),
        "edit" => serde_json::from_str::<EditStatusRequest>(&row.payload_json)
            .map(|request| request.status)
            .unwrap_or_default(),
        _ => String::new(),
    };
    ComposeOutboxItemView {
        id: row.id,
        operation_kind: row.operation_kind,
        acting_account_acct: row.acting_account_acct,
        content_preview: truncate_preview(content.trim(), 160),
        state: row.state,
        attempts: row.attempts,
        last_error: row.last_error,
        next_attempt_at: row.next_attempt_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        completed_at: row.completed_at,
        result_status_id: row.result_status_id,
        result_server_domain: row.result_server_domain,
    }
}

fn truncate_preview(value: &str, maximum: usize) -> String {
    let mut chars = value.chars();
    let preview = chars.by_ref().take(maximum).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
