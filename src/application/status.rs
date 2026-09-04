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
use crate::mastodon::types::status::Status;
use crate::observability::OperationContext;
use crate::plugins::{PluginHook, PluginHookToken, PluginManager};
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

fn status_action_hooks(action: &str) -> Option<(PluginHook, PluginHook)> {
    match action {
        "favourite" | "unfavourite" => {
            Some((PluginHook::BeforeFavorite, PluginHook::AfterFavorite))
        }
        "reblog" | "unreblog" => Some((PluginHook::BeforeBoost, PluginHook::AfterBoost)),
        "bookmark" | "unbookmark" => Some((PluginHook::BeforeBookmark, PluginHook::AfterBookmark)),
        _ => None,
    }
}

fn validate_create_post_visibility(visibility: Option<&str>) -> Result<(), String> {
    match visibility {
        None | Some("public" | "unlisted" | "private" | "direct") => Ok(()),
        Some(value) => Err(format!("Unsupported post visibility: {value}")),
    }
}

fn delete_cleanup_targets(
    original: (String, String),
    provider: (String, String),
    transformed: Option<(String, String)>,
) -> (Vec<(String, String)>, usize) {
    let mut targets = vec![original];
    if !targets.contains(&provider) {
        targets.push(provider);
    }
    let required_count = targets.len();
    if let Some(transformed) = transformed {
        if !targets.contains(&transformed) {
            targets.push(transformed);
        }
    }
    (targets, required_count)
}

fn classify_delete_hook_snapshot<T>(
    has_before_hook: bool,
    snapshot: Result<T, String>,
) -> Result<Option<T>, String> {
    match snapshot {
        Ok(status) => Ok(Some(status)),
        Err(error) if has_before_hook => Err(error),
        Err(error) => {
            tracing::warn!(
                hook = "afterDeletePost",
                %error,
                "Failed to fetch the optional after-delete snapshot; continuing with deletion"
            );
            Ok(None)
        }
    }
}

fn before_create_plugin_error(error: impl std::fmt::Display, request_id: &str) -> AppError {
    // This entire stage runs before `create_status`. Classifying a plugin
    // failure as Internal/Timeout would make a non-idempotent outbox delivery
    // look provider-ambiguous even though no remote mutation was attempted.
    AppError::from_code(AppErrorCode::Validation, error, request_id)
        .with_safe_detail("field", "plugin")
}

async fn plugin_has_hook(plugins: &PluginManager, hook: PluginHook) -> Result<bool, String> {
    let plugins = plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.has_hook(hook))
        .await
        .map_err(|error| format!("plugin hook availability task failed: {error}"))?
}

async fn plugin_hook_token(
    plugins: &PluginManager,
    hook: PluginHook,
) -> Result<Option<PluginHookToken>, String> {
    let plugins = plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.hook_token(hook))
        .await
        .map_err(|error| format!("plugin hook token task failed: {error}"))?
}

async fn run_plugin_hook(
    plugins: &PluginManager,
    hook: PluginHook,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let plugins = plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.run_hook(hook, &value))
        .await
        .map_err(|error| format!("plugin hook task failed: {error}"))?
}

async fn run_plugin_hook_checked(
    plugins: &PluginManager,
    hook: PluginHook,
    token: PluginHookToken,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let plugins = plugins.clone();
    tauri::async_runtime::spawn_blocking(move || plugins.run_hook_checked(hook, token, &value))
        .await
        .map_err(|error| format!("checked plugin hook task failed: {error}"))?
}

async fn run_plugin_hook_best_effort(
    plugins: &PluginManager,
    hook: PluginHook,
    value: serde_json::Value,
) -> serde_json::Value {
    let fallback = value.clone();
    let plugins = plugins.clone();
    match tauri::async_runtime::spawn_blocking(move || plugins.run_hook_best_effort(hook, &value))
        .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(
                ?hook,
                %error,
                "Plugin after-hook worker task failed; using the unmodified payload"
            );
            fallback
        }
    }
}

fn post_request_plugin_payload(request: &PostRequest) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(request)
        .map_err(|error| format!("Failed to serialize CreatePost hook input: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "CreatePost hook input is not an object".to_string())?;
    let text = object
        .remove("status")
        .ok_or_else(|| "CreatePost hook input is missing text".to_string())?;
    object.insert("text".to_string(), text);
    object.insert(
        "_awayukiAction".to_string(),
        serde_json::Value::String("create".to_string()),
    );
    object.insert(
        "_awayukiActingAccountAcct".to_string(),
        serde_json::Value::String(request.acting_account_acct.clone()),
    );
    Ok(value)
}

fn post_request_from_plugin_result(
    mut value: serde_json::Value,
    operation_id: Option<&str>,
    acting_account_acct: &str,
) -> Result<PostRequest, String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "CreatePost hook must return an object".to_string())?;
    let text = object
        .remove("text")
        .ok_or_else(|| "CreatePost hook result is missing text".to_string())?;
    object.insert("status".to_string(), text);
    object.insert(
        "operationId".to_string(),
        operation_id
            .map(|value| serde_json::Value::String(value.to_string()))
            .unwrap_or(serde_json::Value::Null),
    );
    object.insert(
        "actingAccountAcct".to_string(),
        serde_json::Value::String(acting_account_acct.to_string()),
    );
    serde_json::from_value(value)
        .map_err(|error| format!("CreatePost hook returned an invalid post: {error}"))
}

fn status_plugin_payload(
    status: &Status,
    action: &str,
    acting_account_acct: &str,
) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(status)
        .map_err(|error| format!("Failed to serialize status hook input: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Status hook input is not an object".to_string())?;
    object.insert(
        "_awayukiAction".to_string(),
        serde_json::Value::String(action.to_string()),
    );
    object.insert(
        "_awayukiActingAccountAcct".to_string(),
        serde_json::Value::String(acting_account_acct.to_string()),
    );
    Ok(value)
}

async fn run_status_hook_checked(
    plugins: &PluginManager,
    hook: PluginHook,
    token: PluginHookToken,
    status: &Status,
    action: &str,
    acting_account_acct: &str,
) -> Result<Status, String> {
    let payload = status_plugin_payload(status, action, acting_account_acct)?;
    let result = run_plugin_hook_checked(plugins, hook, token, payload).await?;
    let status: Status = serde_json::from_value(result)
        .map_err(|error| format!("Status hook returned an invalid status: {error}"))?;
    if status.id.trim().is_empty() {
        return Err("Status hook returned an empty status id".to_string());
    }
    Ok(status)
}

async fn run_after_status_hook(
    plugins: &PluginManager,
    hook: PluginHook,
    status: Status,
    action: &str,
    acting_account_acct: &str,
    hook_name: &'static str,
) -> Status {
    let payload = match status_plugin_payload(&status, action, acting_account_acct) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!(
                hook = hook_name,
                action,
                %error,
                "Failed to serialize an after hook input; using the unmodified status"
            );
            return status;
        }
    };
    let result = run_plugin_hook_best_effort(plugins, hook, payload).await;
    match serde_json::from_value::<Status>(result) {
        Ok(transformed) if !transformed.id.trim().is_empty() => transformed,
        Ok(_) => {
            tracing::warn!(
                hook = hook_name,
                action,
                "Plugin after hook returned an empty status id; using the unmodified status"
            );
            status
        }
        Err(error) => {
            tracing::warn!(
                hook = hook_name,
                action,
                %error,
                "Plugin after hook returned an invalid status; using the unmodified status"
            );
            status
        }
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
    let result = post_status_inner(state.inner(), request, &operation, false).await;
    match result {
        Ok(status) => {
            operation.finish_ok();
            Ok(status)
        }
        Err(error) => Err(operation.finish_app_error(error)),
    }
}

async fn post_status_inner(
    state: &RuntimeState,
    mut request: PostRequest,
    operation: &OperationContext,
    await_cache_commit: bool,
) -> Result<TimelineStatus, AppError> {
    let immutable_operation_id = request.operation_id.clone();
    let immutable_acting_account_acct = request.acting_account_acct.clone();
    let original_status_text = request.status.trim().to_string();
    let preset_visibility = settings_application::load_setting::<PresetVisibilitySettings>(
        state.database(),
        "preset_visibility",
    )
    .await
    .map_err(|error| AppError::from_source(error, operation.id()))?
    .match_visibility(&original_status_text)
    .map(|visibility| visibility.as_request_visibility().to_string());
    // The frontend already resolves the normal preset path. An explicit
    // request value (including one returned by a compose plugin button) must
    // remain authoritative; callers that omit visibility still get the
    // backend preset fallback.
    request.visibility = request.visibility.take().or(preset_visibility);

    operation.phase("plugin_before");
    let payload = post_request_plugin_payload(&request)
        .map_err(|error| before_create_plugin_error(error, operation.id()))?;
    let result = run_plugin_hook(&state.plugins, PluginHook::BeforeCreatePost, payload)
        .await
        .map_err(|error| before_create_plugin_error(error, operation.id()))?;
    request = post_request_from_plugin_result(
        result,
        immutable_operation_id.as_deref(),
        &immutable_acting_account_acct,
    )
    .map_err(|error| before_create_plugin_error(error, operation.id()))?;

    validate_create_post_visibility(request.visibility.as_deref()).map_err(|error| {
        AppError::from_code(AppErrorCode::Validation, error, operation.id())
            .with_safe_detail("field", "visibility")
    })?;

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
    let session = acting_session(state, &request.acting_account_acct)
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
    if (request.quote_id.is_some() || request.quote_identity.is_some())
        && !capabilities.compose.quote
    {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    let in_reply_to_id = match request.in_reply_to_identity.as_ref() {
        Some(identity) => Some(
            resolve_status_id_for_acting_account(&session, identity)
                .await
                .map_err(|error| AppError::from_source(error, operation.id()))?,
        ),
        None => request.in_reply_to_id,
    };
    let quote_id = match request.quote_identity.as_ref() {
        Some(identity) => Some(
            resolve_status_id_for_acting_account(&session, identity)
                .await
                .map_err(|error| AppError::from_source(error, operation.id()))?,
        ),
        None => request.quote_id,
    };
    let client = session.client;
    operation.phase("api");
    let status = client
        .create_status(&CreateStatusParams {
            idempotency_key: Some(operation.id().to_string()),
            status: (!status_text.is_empty()).then_some(request.status),
            in_reply_to_id,
            media_ids,
            sensitive: request.sensitive,
            spoiler_text: request.spoiler_text,
            visibility: request.visibility,
            language: None,
            quote_id,
            poll,
        })
        .await
        .map_err(|error| AppError::from_adapter(error, operation.id()))?;
    operation.phase("plugin_after");
    let status = run_after_status_hook(
        &state.plugins,
        PluginHook::AfterCreatePost,
        status,
        "create",
        &session.acct,
        "afterCreatePost",
    )
    .await;
    operation.phase("commit");

    let server_domain = client.domain().to_string();
    let source_acct = session.acct;
    let posted = with_source_acct(
        status_to_view(&status, &server_domain, None),
        Some(source_acct.clone()),
    );
    let runtime = state.clone();
    let operation_id = operation.id().to_string();
    let cache_write = cache_posted_status(
        runtime,
        client,
        status,
        source_acct,
        server_domain,
        operation_id,
    );
    if await_cache_commit {
        cache_write.await;
    } else {
        // Foreground compatibility command preserves its existing provider
        // response boundary. The durable outbox path awaits this same future
        // before marking its item succeeded.
        tauri::async_runtime::spawn(cache_write);
    }

    Ok(posted)
}

async fn cache_posted_status(
    runtime: RuntimeState,
    client: crate::api::client::ApiClient,
    status: crate::mastodon::types::status::Status,
    source_acct: String,
    server_domain: String,
    operation_id: String,
) {
    let started_at = Instant::now();
    let items = [timeline_service::StatusBatchItem {
        status: &status,
        timeline: Some(timeline_service::BatchTimeline {
            timeline_type: "home",
            account_acct: &source_acct,
        }),
        viewer_acct: Some(&source_acct),
    }];
    match timeline_service::save_status_items_with_retry(
        runtime.database().writer(),
        &items,
        &server_domain,
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
}

pub(crate) async fn deliver_queued_post(
    state: &RuntimeState,
    mut request: PostRequest,
    outbox_id: &str,
) -> Result<TimelineStatus, AppError> {
    request.operation_id = Some(outbox_id.to_string());
    let mut operation = OperationContext::start(
        "compose_outbox_post",
        Some(outbox_id),
        Some(&request.acting_account_acct),
    );
    let result = post_status_inner(state, request, &operation, true).await;
    match result {
        Ok(status) => {
            operation.finish_ok();
            Ok(status)
        }
        Err(error) => Err(operation.finish_app_error(error)),
    }
}

pub(crate) async fn status_action(
    state: State<'_, RuntimeState>,
    request: StatusActionRequest,
) -> Result<TimelineStatus, String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    let operation = status_operation(&request.action)?;
    let (before_hook, after_hook) = status_action_hooks(&request.action)
        .ok_or_else(|| format!("Unsupported status action: {}", request.action))?;
    session
        .client
        .capabilities(1)
        .require_status(operation)
        .map_err(|e| e.to_string())?;
    let mut remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let client = session.client;
    let acting_acct = session.acct;

    if let Some(before_token) = plugin_hook_token(&state.plugins, before_hook).await? {
        let target = client
            .get_status(&remote_id)
            .await
            .map_err(|error| error.to_string())?;
        let transformed = run_status_hook_checked(
            &state.plugins,
            before_hook,
            before_token,
            &target,
            &request.action,
            &acting_acct,
        )
        .await?;
        remote_id = transformed.id;
    }

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
    let status = run_after_status_hook(
        &state.plugins,
        after_hook,
        status,
        &request.action,
        &acting_acct,
        match request.action.as_str() {
            "favourite" | "unfavourite" => "afterFavorite",
            "reblog" | "unreblog" => "afterBoost",
            "bookmark" | "unbookmark" => "afterBookmark",
            _ => unreachable!("validated status action"),
        },
    )
    .await;
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
    edit_own_status_inner(state.inner(), request).await
}

async fn edit_own_status_inner(
    state: &RuntimeState,
    request: EditStatusRequest,
) -> Result<TimelineStatus, String> {
    let status_text = request.status.trim().to_string();
    if status_text.is_empty() {
        return Err("Post text is empty".to_string());
    }
    let session = acting_session(state, &request.acting_account_acct).await?;
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

pub(crate) async fn deliver_queued_edit(
    state: &RuntimeState,
    request: EditStatusRequest,
    outbox_id: &str,
) -> Result<TimelineStatus, AppError> {
    let mut operation = OperationContext::start(
        "compose_outbox_edit",
        Some(outbox_id),
        Some(&request.acting_account_acct),
    );
    match edit_own_status_inner(state, request).await {
        Ok(status) => {
            operation.finish_ok();
            Ok(status)
        }
        Err(error) => Err(operation.finish_app_error(AppError::from_source(error, outbox_id))),
    }
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
    let mut remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let before_hook_token = plugin_hook_token(&state.plugins, PluginHook::BeforeDeletePost).await?;
    let has_before_hook = before_hook_token.is_some();
    let has_after_hook = match plugin_has_hook(&state.plugins, PluginHook::AfterDeletePost).await {
        Ok(has_hook) => has_hook,
        Err(error) => {
            tracing::warn!(
                hook = "afterDeletePost",
                %error,
                "Failed to check an after hook; continuing with the provider deletion"
            );
            false
        }
    };
    let mut hook_status = if has_before_hook || has_after_hook {
        classify_delete_hook_snapshot(
            has_before_hook,
            session
                .client
                .get_status(&remote_id)
                .await
                .map_err(|error| error.to_string()),
        )?
    } else {
        None
    };

    if let Some(before_token) = before_hook_token {
        let status = hook_status
            .as_ref()
            .ok_or_else(|| "Delete hook target status was not fetched".to_string())?;
        let transformed = run_status_hook_checked(
            &state.plugins,
            PluginHook::BeforeDeletePost,
            before_token,
            status,
            "delete",
            &session.acct,
        )
        .await?;
        remote_id = transformed.id.clone();
        hook_status = Some(transformed);
    }

    let provider_delete_id = remote_id.clone();
    session
        .client
        .delete_status(&provider_delete_id)
        .await
        .map_err(|e| e.to_string())?;
    if has_after_hook {
        if let Some(status) = hook_status.take() {
            hook_status = Some(
                run_after_status_hook(
                    &state.plugins,
                    PluginHook::AfterDeletePost,
                    status,
                    "delete",
                    &session.acct,
                    "afterDeletePost",
                )
                .await,
            );
        } else {
            tracing::warn!(
                hook = "afterDeletePost",
                "Delete target snapshot was unavailable after the provider mutation"
            );
        }
    }
    let original_cleanup_target = (request.identity.remote_id, request.identity.server_domain);
    let transformed_cleanup_target =
        hook_status.map(|status| (status.id, session.client.domain().to_string()));
    let (cleanup_targets, required_count) = delete_cleanup_targets(
        original_cleanup_target,
        (provider_delete_id, session.client.domain().to_string()),
        transformed_cleanup_target,
    );
    let mut required_cleanup_error = None;
    for (index, (status_id, server_domain)) in cleanup_targets.into_iter().enumerate() {
        if let Err(error) = crate::db::queries::statuses::delete_status_and_references(
            state.database().writer(),
            &status_id,
            &server_domain,
        )
        .await
        {
            let is_required = index < required_count;
            let error_message = error.to_string();
            tracing::warn!(
                status_id,
                server_domain,
                is_required,
                %error,
                "Failed to clean a status target after the provider deletion"
            );
            if is_required && required_cleanup_error.is_none() {
                required_cleanup_error = Some(error_message);
            }
        }
    }
    if let Some(error) = required_cleanup_error {
        return Err(error);
    }
    state.emit_timeline_cache_committed(&session.acct, session.client.domain());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post_request_fixture() -> PostRequest {
        PostRequest {
            operation_id: Some("operation-original".to_string()),
            acting_account_acct: "alice@example.com".to_string(),
            status: "original text".to_string(),
            visibility: Some("public".to_string()),
            spoiler_text: None,
            sensitive: None,
            media_ids: None,
            in_reply_to_id: None,
            in_reply_to_identity: None,
            quote_id: None,
            quote_identity: None,
            poll: None,
        }
    }

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

    #[test]
    fn inverse_actions_use_the_same_plugin_hook_category() {
        assert!(matches!(
            status_action_hooks("unreblog"),
            Some((PluginHook::BeforeBoost, PluginHook::AfterBoost))
        ));
        assert!(matches!(
            status_action_hooks("unfavourite"),
            Some((PluginHook::BeforeFavorite, PluginHook::AfterFavorite))
        ));
        assert!(matches!(
            status_action_hooks("unbookmark"),
            Some((PluginHook::BeforeBookmark, PluginHook::AfterBookmark))
        ));
    }

    #[test]
    fn create_post_visibility_rejects_unknown_values_before_misskey_fallback() {
        for valid in [
            None,
            Some("public"),
            Some("unlisted"),
            Some("private"),
            Some("direct"),
        ] {
            assert!(validate_create_post_visibility(valid).is_ok());
        }

        let mut misskey_adapter_called = false;
        let result = validate_create_post_visibility(Some("plugin-only")).map(|()| {
            misskey_adapter_called = true;
            crate::misskey::convert::visibility_to_misskey("plugin-only")
        });

        assert!(result.is_err());
        assert!(!misskey_adapter_called);
    }

    #[test]
    fn before_create_plugin_failures_are_unambiguous_for_the_outbox() {
        let error = before_create_plugin_error("plugin fetch timeout", "outbox-1");

        assert_eq!(error.code, AppErrorCode::Validation);
        assert!(!matches!(
            error.code,
            AppErrorCode::Timeout | AppErrorCode::Internal
        ));
        assert_eq!(
            error.safe_details.get("field").map(String::as_str),
            Some("plugin")
        );
    }

    #[test]
    fn delete_cleanup_keeps_original_provider_and_final_hook_targets() {
        let (targets, required_count) = delete_cleanup_targets(
            ("status-a".to_string(), "origin.example".to_string()),
            ("status-b".to_string(), "acting.example".to_string()),
            Some(("status-c".to_string(), "acting.example".to_string())),
        );

        assert_eq!(required_count, 2);
        assert_eq!(
            targets,
            vec![
                ("status-a".to_string(), "origin.example".to_string()),
                ("status-b".to_string(), "acting.example".to_string()),
                ("status-c".to_string(), "acting.example".to_string()),
            ]
        );
    }

    #[test]
    fn delete_snapshot_failure_is_strict_only_for_before_hook() {
        let before_error =
            classify_delete_hook_snapshot::<()>(true, Err("snapshot unavailable".to_string()));
        let after_only =
            classify_delete_hook_snapshot::<()>(false, Err("snapshot unavailable".to_string()));

        assert_eq!(before_error.unwrap_err(), "snapshot unavailable");
        assert!(after_only
            .expect("after-only snapshot is optional")
            .is_none());
        assert_eq!(
            classify_delete_hook_snapshot(false, Ok("snapshot")).expect("available snapshot"),
            Some("snapshot")
        );
    }

    #[test]
    fn create_post_hook_uses_text_alias_and_action_metadata() {
        let request = post_request_fixture();
        let payload = post_request_plugin_payload(&request).expect("serialize hook payload");

        assert_eq!(payload["text"], "original text");
        assert!(payload.get("status").is_none());
        assert_eq!(payload["visibility"], "public");
        assert_eq!(payload["_awayukiAction"], "create");
        assert_eq!(payload["_awayukiActingAccountAcct"], "alice@example.com");
    }

    #[test]
    fn create_post_hook_result_drives_text_but_not_actor_or_operation_id() {
        let request = post_request_fixture();
        let mut result = post_request_plugin_payload(&request).expect("serialize hook payload");
        let object = result.as_object_mut().expect("hook payload object");
        object.insert(
            "text".to_string(),
            serde_json::Value::String("plugin text".to_string()),
        );
        object.insert(
            "visibility".to_string(),
            serde_json::Value::String("private".to_string()),
        );
        object.insert(
            "operationId".to_string(),
            serde_json::Value::String("plugin-operation".to_string()),
        );
        object.insert(
            "actingAccountAcct".to_string(),
            serde_json::Value::String("mallory@example.net".to_string()),
        );

        let transformed = post_request_from_plugin_result(
            result,
            request.operation_id.as_deref(),
            &request.acting_account_acct,
        )
        .expect("deserialize hook result");

        assert_eq!(transformed.status, "plugin text");
        assert_eq!(transformed.visibility.as_deref(), Some("private"));
        assert_eq!(
            transformed.operation_id.as_deref(),
            Some("operation-original")
        );
        assert_eq!(transformed.acting_account_acct, "alice@example.com");
    }
}
