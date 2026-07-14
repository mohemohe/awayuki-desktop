//! Status mutations with explicit actor and status identity.
//!
//! `acting_account_acct` always identifies the account performing a mutation.
//! It is never inferred from a Timeline selection or used to narrow Unified
//! Timeline sources.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::State;

use crate::application::desktop::{
    acting_session, poll_to_view, status_to_view, update_cached_status_poll, with_source_acct,
    PollView, RuntimeState, TimelineStatus,
};
use crate::application::settings as settings_application;
use crate::auth::session::AccountSession;
use crate::domain::capability::StatusOperation;
use crate::domain::identity::{FederationProtocol, StatusIdentity};
use crate::ipc::dto::{
    DeleteStatusRequest, EditStatusRequest, PostRequest, StatusActionRequest, VotePollRequest,
};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::mastodon::endpoints::statuses::{CreatePollParams, CreateStatusParams, VotePollParams};
use crate::observability::OperationContext;
use crate::services::timeline_service;
use crate::state::preset_visibility::PresetVisibilitySettings;

#[derive(Debug, Clone)]
struct ResolvedStatusId {
    remote_id: String,
    expires_at: Instant,
}

type ResolvedStatusCacheKey = (String, FederationProtocol, String, String);

fn resolved_status_cache() -> &'static Mutex<HashMap<ResolvedStatusCacheKey, ResolvedStatusId>> {
    static CACHE: OnceLock<Mutex<HashMap<ResolvedStatusCacheKey, ResolvedStatusId>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn status_operation(action: &str) -> Result<StatusOperation, String> {
    match action {
        "favourite" => Ok(StatusOperation::Favourite),
        "unfavourite" => Ok(StatusOperation::Unfavourite),
        "reblog" => Ok(StatusOperation::Reblog),
        "unreblog" => Ok(StatusOperation::Unreblog),
        "bookmark" => Ok(StatusOperation::Bookmark),
        "unbookmark" => Ok(StatusOperation::Unbookmark),
        other => Err(format!("Unsupported status action: {other}")),
    }
}

async fn resolve_status_id_for_acting_account(
    session: &AccountSession,
    identity: &StatusIdentity,
) -> Result<String, String> {
    identity.validate().map_err(|error| error.to_string())?;
    let capabilities = session.client.capabilities(1);
    if capabilities.protocol != identity.protocol {
        return Err("Acting account protocol cannot address this status identity".to_string());
    }
    if session
        .client
        .domain()
        .eq_ignore_ascii_case(&identity.server_domain)
    {
        return Ok(identity.remote_id.clone());
    }

    let key = (
        session.acct.clone(),
        identity.protocol,
        identity.server_domain.clone(),
        identity.canonical_uri.clone(),
    );
    let now = Instant::now();
    if let Some(cached) = resolved_status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .filter(|entry| entry.expires_at > now)
        .cloned()
    {
        return Ok(cached.remote_id);
    }
    let resolved = session
        .client
        .lookup_status_by_uri(&identity.canonical_uri)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "Status is not available to acting account {}: {}",
                session.acct, identity.canonical_uri
            )
        })?;
    let remote_id = resolved.id;
    let mut cache = resolved_status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(
        key,
        ResolvedStatusId {
            remote_id: remote_id.clone(),
            expires_at: now + Duration::from_secs(5 * 60),
        },
    );
    crate::observability::set_cache_entries(cache.len());
    Ok(remote_id)
}

pub(crate) async fn post_status(
    state: State<'_, RuntimeState>,
    request: PostRequest,
) -> Result<TimelineStatus, AppError> {
    let mut operation = OperationContext::start(
        "post_status",
        request.operation_id.as_deref(),
        Some(&request.acting_account_acct),
    );
    let result = post_status_inner(state, request, &operation).await;
    match result {
        Ok(status) => {
            operation.finish_ok();
            Ok(status)
        }
        Err(error) => Err(operation.finish_app_error(error)),
    }
}

async fn post_status_inner(
    state: State<'_, RuntimeState>,
    request: PostRequest,
    operation: &OperationContext,
) -> Result<TimelineStatus, AppError> {
    let status_text = request.status.trim().to_string();
    let media_ids = request.media_ids.filter(|ids| !ids.is_empty());
    let poll = request.poll.and_then(|poll| {
        let options = poll
            .options
            .into_iter()
            .map(|option| option.trim().to_string())
            .filter(|option| !option.is_empty())
            .collect::<Vec<_>>();
        (options.len() >= 2).then_some(CreatePollParams {
            options,
            expires_in: poll.expires_in,
            multiple: Some(poll.multiple),
            hide_totals: None,
        })
    });
    if status_text.is_empty() && media_ids.is_none() && poll.is_none() {
        return Err(AppError::validation(operation.id()));
    }
    let preset_visibility = settings_application::load_setting::<PresetVisibilitySettings>(
        state.database(),
        "preset_visibility",
    )
    .await
    .map_err(|error| AppError::from_source(error, operation.id()))?
    .match_visibility(&status_text)
    .map(|visibility| visibility.as_request_visibility().to_string());
    let session = acting_session(&state, &request.acting_account_acct)
        .await
        .map_err(|error| AppError::from_source(error, operation.id()))?;
    let capabilities = session.client.capabilities(1);
    if media_ids.as_ref().is_some_and(|ids| !ids.is_empty()) && !capabilities.compose.media_upload {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    if media_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > capabilities.compose.max_media_attachments as usize)
    {
        return Err(AppError::validation(operation.id()));
    }
    if poll.is_some() && !capabilities.compose.poll {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    if request.quote_id.is_some() && !capabilities.compose.quote {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    let client = session.client;
    operation.phase("api");
    let status = client
        .create_status(&CreateStatusParams {
            idempotency_key: Some(operation.id().to_string()),
            status: (!status_text.is_empty()).then_some(request.status),
            in_reply_to_id: request.in_reply_to_id,
            media_ids,
            sensitive: request.sensitive,
            spoiler_text: request.spoiler_text,
            visibility: preset_visibility.or(request.visibility),
            language: None,
            quote_id: request.quote_id,
            poll,
        })
        .await
        .map_err(|error| AppError::from_adapter(error, operation.id()))?;
    operation.phase("commit");

    // The provider accepting the post is the mutation's commit boundary.  The
    // same status can arrive through streaming before the local cache writer is
    // available, so awaiting SQLite here leaves the composer locked even though
    // the post is already visible in Home.  Cache persistence is idempotent and
    // is allowed to finish behind the IPC response.
    let server_domain = client.domain().to_string();
    let source_acct = session.acct;
    let posted = status_to_view(&status, &server_domain, None);
    let runtime = state.inner().clone();
    let operation_id = operation.id().to_string();
    tauri::async_runtime::spawn(async move {
        let started_at = Instant::now();
        match timeline_service::save_status_for_viewer_to_db_with_retry(
            runtime.database().writer(),
            &status,
            &server_domain,
            &source_acct,
        )
        .await
        {
            Ok(()) => {
                timeline_service::schedule_pending_quote_resolution(
                    &client,
                    runtime.database().writer(),
                    std::slice::from_ref(&status),
                    &server_domain,
                    &source_acct,
                );
                let duration_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
                crate::observability::observe_db_query(1, duration_ms);
                runtime.emit_timeline_cache_committed(&source_acct, &server_domain);
                tracing::debug!(
                    operation_id,
                    duration_ms,
                    "Posted status cache write completed"
                );
            }
            Err(error) => {
                tracing::warn!(
                    operation_id,
                    duration_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    %error,
                    "Failed to cache status after the provider accepted the post"
                );
            }
        }
    });

    Ok(posted)
}

pub(crate) async fn status_action(
    state: State<'_, RuntimeState>,
    request: StatusActionRequest,
) -> Result<TimelineStatus, String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    let operation = status_operation(&request.action)?;
    session
        .client
        .capabilities(1)
        .require_status(operation)
        .map_err(|e| e.to_string())?;
    let remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let client = session.client;
    let acting_acct = session.acct;
    let status = match request.action.as_str() {
        "favourite" => client.favourite(&remote_id).await,
        "unfavourite" => client.unfavourite(&remote_id).await,
        "reblog" => client.reblog(&remote_id).await,
        "unreblog" => client.unreblog(&remote_id).await,
        "bookmark" => client.bookmark(&remote_id).await,
        "unbookmark" => client.unbookmark(&remote_id).await,
        _ => unreachable!("validated status action"),
    }
    .map_err(|error| error.to_string())?;
    timeline_service::save_status_for_viewer_to_db_with_retry(
        state.database().writer(),
        &status,
        client.domain(),
        &acting_acct,
    )
    .await
    .map_err(|error| error.to_string())?;
    state.emit_timeline_cache_committed(&acting_acct, client.domain());
    timeline_service::schedule_pending_quote_resolution(
        &client,
        state.database().writer(),
        std::slice::from_ref(&status),
        client.domain(),
        &acting_acct,
    );

    let timeline_type = match request.action.as_str() {
        "favourite" | "unfavourite" => Some("favourites"),
        "bookmark" | "unbookmark" => Some("bookmarks"),
        _ => None,
    };
    if let Some(timeline_type) = timeline_type {
        if matches!(request.action.as_str(), "favourite" | "bookmark") {
            timeline_service::insert_timeline_entry_with_retry(
                state.database().writer(),
                timeline_type,
                client.domain(),
                &status.id,
                &acting_acct,
                &status.created_at.to_rfc3339(),
            )
            .await
            .map_err(|error| error.to_string())?;
        } else {
            sqlx::query(
                "DELETE FROM timeline_entries WHERE timeline_type = ? AND status_id = ? AND server_domain = ? AND account_acct = ?",
            )
            .bind(timeline_type)
            .bind(&status.id)
            .bind(client.domain())
            .bind(&acting_acct)
            .execute(state.database().writer())
            .await
            .map_err(|error| error.to_string())?;
        }
    }
    state.emit_timeline_cache_committed(&acting_acct, client.domain());
    Ok(with_source_acct(
        status_to_view(&status, client.domain(), None),
        Some(acting_acct),
    ))
}

pub(crate) async fn vote_poll(
    state: State<'_, RuntimeState>,
    request: VotePollRequest,
) -> Result<PollView, String> {
    if request.choices.is_empty() {
        return Err("Select at least one poll option".to_string());
    }
    let session = acting_session(&state, &request.acting_account_acct).await?;
    session
        .client
        .capabilities(1)
        .require_status(StatusOperation::Vote)
        .map_err(|e| e.to_string())?;
    let remote_status_id =
        resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let remote_poll_id = if session
        .client
        .domain()
        .eq_ignore_ascii_case(&request.identity.server_domain)
    {
        request.poll_id.clone()
    } else {
        session
            .client
            .get_status(&remote_status_id)
            .await
            .map_err(|e| e.to_string())?
            .poll
            .map(|poll| poll.id)
            .ok_or_else(|| "Resolved remote status has no poll".to_string())?
    };
    let poll = session
        .client
        .vote_poll(
            &remote_poll_id,
            &VotePollParams {
                choices: request.choices.clone(),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(poll_json) = serde_json::to_string(&poll) {
        match update_cached_status_poll(state.database().writer(), &request, &poll_json).await {
            Ok(()) => {
                state.emit_timeline_cache_committed(&session.acct, session.client.domain());
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to update cached poll {}: {}",
                    request.poll_id,
                    error
                );
            }
        }
    }
    Ok(poll_to_view(&poll))
}

pub(crate) async fn edit_own_status(
    state: State<'_, RuntimeState>,
    request: EditStatusRequest,
) -> Result<TimelineStatus, String> {
    let status_text = request.status.trim().to_string();
    if status_text.is_empty() {
        return Err("Post text is empty".to_string());
    }
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if session.account_info.id != request.account_id {
        return Err("Acting account does not own this post".to_string());
    }
    session
        .client
        .capabilities(1)
        .require_status(StatusOperation::Edit)
        .map_err(|e| e.to_string())?;
    let remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let status = session
        .client
        .edit_status(
            &remote_id,
            &CreateStatusParams {
                idempotency_key: None,
                status: Some(status_text),
                in_reply_to_id: None,
                media_ids: None,
                sensitive: request.sensitive,
                spoiler_text: request.spoiler_text,
                visibility: request.visibility,
                language: None,
                quote_id: None,
                poll: None,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    timeline_service::save_status_for_viewer_to_db_with_retry(
        state.database().writer(),
        &status,
        session.client.domain(),
        &session.acct,
    )
    .await
    .map_err(|e| e.to_string())?;
    timeline_service::schedule_pending_quote_resolution(
        &session.client,
        state.database().writer(),
        std::slice::from_ref(&status),
        session.client.domain(),
        &session.acct,
    );
    state.emit_timeline_cache_committed(&session.acct, session.client.domain());
    Ok(status_to_view(&status, session.client.domain(), None))
}

pub(crate) async fn delete_own_status(
    state: State<'_, RuntimeState>,
    request: DeleteStatusRequest,
) -> Result<(), String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if session.account_info.id != request.account_id {
        return Err("Acting account does not own this post".to_string());
    }
    session
        .client
        .capabilities(1)
        .require_status(StatusOperation::Delete)
        .map_err(|e| e.to_string())?;
    let remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    session
        .client
        .delete_status(&remote_id)
        .await
        .map_err(|e| e.to_string())?;
    crate::db::queries::statuses::delete_status_and_references(
        state.database().writer(),
        &request.identity.remote_id,
        &request.identity.server_domain,
    )
    .await
    .map_err(|e| e.to_string())?;
    state.emit_timeline_cache_committed(&session.acct, session.client.domain());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mutation_actions_are_explicit_and_never_timeline_kinds() {
        for action in [
            "favourite",
            "unfavourite",
            "reblog",
            "unreblog",
            "bookmark",
            "unbookmark",
        ] {
            assert!(status_operation(action).is_ok());
        }
        for invalid in ["home", "public", "notification", "active"] {
            assert!(status_operation(invalid).is_err());
        }
    }
}
