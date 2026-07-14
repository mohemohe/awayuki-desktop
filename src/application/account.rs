//! Account lifecycle and account-scoped local preferences.
//!
//! The active account selects the actor for post/boost/favourite operations.
//! It never selects or narrows Home, Public, or Notification timeline sources.
//! Consequently an active-account switch does not restart streaming; logout
//! does, because it changes the set of signed-in sources in Unified Timeline.

use serde::Serialize;
use std::time::Instant;
use tauri::State;

use crate::application::desktop::{
    acting_session, app_snapshot_for_state, db_statuses_to_views, login_accounts,
    query_account_statuses, restart_streaming, run_cancellable_read, session_for_acct,
    session_for_read_source, status_to_view, with_source_acct, AppSnapshot, RuntimeState,
    TimelineStatus,
};
use crate::application::timeline_view::{parse_custom_emoji_views, CustomEmojiView};
use crate::constants::DEFAULT_TIMELINE_LIMIT;
use crate::db::models::DbAccount;
use crate::db::queries::{accounts, notification_mutes, settings};
use crate::domain::capability::{RelationshipOperation, SessionCapabilities};
use crate::ipc::dto::{
    AccountFollowRequest, AccountListsRequest, AccountNotificationMuteRequest,
    AccountProfileRequest, AccountTimelineRequest,
};
use crate::ipc::error::AppError;
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::services::timeline_service;

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountListSummary {
    id: String,
    title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountRelationshipSummary {
    pub(crate) following: bool,
    pub(crate) followed_by: bool,
    pub(crate) requested: bool,
    pub(crate) blocking: bool,
    pub(crate) muting: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountSummary {
    pub(crate) acct: String,
    pub(crate) server_domain: String,
    pub(crate) account_id: String,
    pub(crate) display_name: String,
    pub(crate) avatar: String,
    pub(crate) is_active: bool,
    pub(crate) server_kind: String,
    pub(crate) character_limit: i32,
    pub(crate) rate_limit: Option<AccountRateLimitSummary>,
    pub(crate) capabilities: SessionCapabilities,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountRateLimitSummary {
    pub(crate) limit: u32,
    pub(crate) remaining: u32,
    pub(crate) used: u32,
    pub(crate) reset_in_seconds: i64,
    pub(crate) observed_ago_seconds: i64,
    pub(crate) policy: Option<String>,
    pub(crate) used_fraction: f32,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountFieldSummary {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountProfileSummary {
    id: String,
    server_domain: String,
    username: String,
    acct: String,
    url: Option<String>,
    display_name: String,
    note: String,
    avatar: String,
    header: String,
    fields: Vec<AccountFieldSummary>,
    pub(crate) account_emojis: Vec<CustomEmojiView>,
    statuses_count: i64,
    following_count: i64,
    followers_count: i64,
    is_self: bool,
    relationship: Option<AccountRelationshipSummary>,
    notification_muted: bool,
}

pub(crate) fn account_profile_to_view(
    account: DbAccount,
    is_self: bool,
    relationship: Option<AccountRelationshipSummary>,
    notification_muted: bool,
    url: Option<String>,
) -> AccountProfileSummary {
    let fields = account
        .fields_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<AccountFieldSummary>>(json).ok())
        .unwrap_or_default();

    AccountProfileSummary {
        id: account.id,
        server_domain: account.server_domain,
        username: account.username,
        acct: account.acct,
        url,
        display_name: account.display_name,
        note: account.note,
        avatar: account.avatar,
        header: account.header,
        fields,
        account_emojis: account
            .emojis_json
            .as_deref()
            .map(parse_custom_emoji_views)
            .unwrap_or_default(),
        statuses_count: account.statuses_count,
        following_count: account.following_count,
        followers_count: account.followers_count,
        is_self,
        relationship,
        notification_muted,
    }
}

pub(crate) fn preserve_cached_profile_media(account: &mut DbAccount, cached: &DbAccount) {
    if account.avatar.trim().is_empty() {
        account.avatar.clone_from(&cached.avatar);
    }
    if account.avatar_static.trim().is_empty() {
        account.avatar_static.clone_from(&cached.avatar_static);
    }
    if account.header.trim().is_empty() {
        account.header.clone_from(&cached.header);
    }
    if account.emojis_json.is_none() {
        account.emojis_json.clone_from(&cached.emojis_json);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationMutedAccountSummary {
    account_id: String,
    server_domain: String,
    acct: String,
    display_name: String,
    avatar: String,
    created_at: String,
    updated_at: String,
}

pub(crate) async fn account_summaries(state: &RuntimeState) -> Result<Vec<AccountSummary>, String> {
    login_accounts(state).await
}

pub(crate) async fn account_lists(
    state: &RuntimeState,
    request: AccountListsRequest,
) -> Result<Vec<AccountListSummary>, String> {
    let acct = request.acct.trim();
    if acct.is_empty() {
        return Err("Account is required".to_string());
    }
    let session = session_for_acct(state, acct)
        .await
        .ok_or_else(|| format!("Account is not signed in: {acct}"))?;
    let mut lists = session
        .client
        .get_lists()
        .await
        .map_err(|error| error.to_string())?;
    lists.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(lists
        .into_iter()
        .map(|list| AccountListSummary {
            id: list.id,
            title: list.title,
        })
        .collect())
}

pub(crate) async fn account_profile(
    state: State<'_, RuntimeState>,
    request: AccountProfileRequest,
) -> Result<AccountProfileSummary, AppError> {
    let operation_id = request.operation_id.clone();
    let manager = state.timeline_query_manager().clone();
    run_cancellable_read(
        manager,
        "account_profile",
        operation_id.as_deref(),
        account_profile_inner(state, request),
    )
    .await
}

async fn account_profile_inner(
    state: State<'_, RuntimeState>,
    request: AccountProfileRequest,
) -> Result<AccountProfileSummary, String> {
    let started_at = Instant::now();
    let session = match session_for_read_source(
        &state,
        &request.server_domain,
        request.source_acct.as_deref(),
    )
    .await
    {
        Ok(session) => session,
        Err(error) if request.source_acct.is_none() => {
            tracing::warn!(
                server_domain = request.server_domain.as_str(),
                %error,
                "Profile read source is ambiguous; using SQLite cache only"
            );
            None
        }
        Err(error) => return Err(error),
    };
    let cached_account = accounts::get_account(
        state.database().reader(),
        &request.account_id,
        &request.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;
    let mut account_url = None;
    let mut account = match &session {
        Some(session) => {
            let account = session
                .client
                .get_account(&request.account_id)
                .await
                .map_err(|error| error.to_string())?;
            account_url = Some(account.url.clone());
            let mut fresh_account = DbAccount::from_api(&account, session.client.domain());
            if let Some(cached_account) = cached_account.as_ref() {
                preserve_cached_profile_media(&mut fresh_account, cached_account);
            }
            accounts::upsert_account(state.database().writer(), &fresh_account)
                .await
                .map_err(|error| error.to_string())?;
            state.emit_timeline_cache_committed(&session.acct, session.client.domain());
            fresh_account
        }
        None => cached_account.ok_or_else(|| "Account is not cached".to_string())?,
    };

    if account.server_domain.is_empty() {
        account.server_domain = request.server_domain.clone();
    }
    let relationship = match &session {
        Some(session) if session.account_info.id != request.account_id => session
            .client
            .get_relationships(&[&request.account_id])
            .await
            .ok()
            .and_then(|relationships| relationships.into_iter().next())
            .map(|relationship| AccountRelationshipSummary {
                following: relationship.following,
                followed_by: relationship.followed_by,
                requested: relationship.requested,
                blocking: relationship.blocking,
                muting: relationship.muting,
            }),
        _ => None,
    };
    let is_self = session
        .as_ref()
        .is_some_and(|session| session.account_info.id == request.account_id);
    let notification_muted = notification_mutes::is_account_muted(
        state.database().reader(),
        &request.account_id,
        &request.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;
    let view = account_profile_to_view(
        account,
        is_self,
        relationship,
        notification_muted,
        account_url,
    );
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        source_acct = ?request.source_acct,
        duration_ms = elapsed_ms(started_at),
        "[awayuki][application] account_profile success"
    );
    Ok(view)
}

pub(crate) async fn account_timeline(
    state: State<'_, RuntimeState>,
    request: AccountTimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    let operation_id = request.operation_id.clone();
    let manager = state.timeline_query_manager().clone();
    run_cancellable_read(
        manager,
        "account_timeline",
        operation_id.as_deref(),
        account_timeline_inner(state, request),
    )
    .await
}

async fn account_timeline_inner(
    state: State<'_, RuntimeState>,
    request: AccountTimelineRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let started_at = Instant::now();
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(80) as i64;
    let offset = request.offset.unwrap_or(0) as i64;
    if request.pinned == Some(true) && offset == 0 {
        let session = match session_for_read_source(
            &state,
            &request.server_domain,
            request.source_acct.as_deref(),
        )
        .await
        {
            Ok(session) => session,
            Err(error) if request.source_acct.is_none() => {
                tracing::warn!(
                    server_domain = request.server_domain.as_str(),
                    %error,
                    "Pinned profile read source is ambiguous; using SQLite cache only"
                );
                None
            }
            Err(error) => return Err(error),
        };
        if let Some(session) = session {
            let statuses = session
                .client
                .get_account_statuses(
                    &request.account_id,
                    &AccountStatusesParams {
                        pinned: Some(true),
                        limit: Some(limit as u32),
                        exclude_replies: Some(false),
                        exclude_reblogs: Some(false),
                        only_media: request.only_media,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            let mut on_commit = || {
                state.emit_timeline_cache_committed(&session.acct, session.client.domain());
            };
            timeline_service::save_status_batch_with_commit_observer(
                state.database().writer(),
                &statuses,
                session.client.domain(),
                None,
                &mut on_commit,
            )
            .await
            .map_err(|error| error.to_string())?;
            if let Some(consumer_id) = request.quote_consumer_id.as_deref() {
                timeline_service::schedule_pending_quote_resolution_for_consumer(
                    &session.client,
                    state.database().writer(),
                    &statuses,
                    session.client.domain(),
                    &session.acct,
                    consumer_id,
                );
            } else {
                timeline_service::schedule_pending_quote_resolution(
                    &session.client,
                    state.database().writer(),
                    &statuses,
                    session.client.domain(),
                    &session.acct,
                );
            }
            let views = statuses
                .iter()
                .map(|status| {
                    with_source_acct(
                        status_to_view(status, session.client.domain(), None),
                        Some(session.acct.clone()),
                    )
                })
                .collect::<Vec<_>>();
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = request.server_domain.as_str(),
                source_acct = session.acct.as_str(),
                count = views.len(),
                duration_ms = elapsed_ms(started_at),
                "[awayuki][application] account_timeline success source=api"
            );
            return Ok(views);
        }
    }

    let statuses = query_account_statuses(
        state.database().reader(),
        &request.account_id,
        &request.server_domain,
        request.only_media.unwrap_or(false),
        request.pinned,
        limit,
        offset,
    )
    .await?;
    let views = db_statuses_to_views(state.database().reader(), statuses).await?;
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        source_acct = ?request.source_acct,
        count = views.len(),
        duration_ms = elapsed_ms(started_at),
        "[awayuki][application] account_timeline success source=db"
    );
    Ok(views)
}

pub(crate) async fn follow_action(
    state: &RuntimeState,
    request: AccountFollowRequest,
) -> Result<AccountRelationshipSummary, String> {
    // Acting account is explicit and never inferred from the active Timeline.
    let session = acting_session(state, &request.acting_account_acct).await?;
    let operation = relationship_operation(&request.action)?;
    session
        .client
        .capabilities(1)
        .require_relationship(operation)
        .map_err(|error| error.to_string())?;
    let target_account_id = if session
        .client
        .domain()
        .eq_ignore_ascii_case(&request.server_domain)
    {
        request.account_id.clone()
    } else {
        let target_acct = request.target_acct.trim().trim_start_matches('@');
        if target_acct.is_empty() {
            return Err("targetAcct is required for a remote relationship action".to_string());
        }
        session
            .client
            .search_accounts(target_acct, 20)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|account| {
                account
                    .acct
                    .trim_start_matches('@')
                    .eq_ignore_ascii_case(target_acct)
                    || account.id == request.account_id
            })
            .map(|account| account.id)
            .ok_or_else(|| {
                format!(
                    "Remote account is not available to acting account {}: {}",
                    session.acct, request.target_acct
                )
            })?
    };
    let relationship = match request.action.as_str() {
        "follow" => session.client.follow_account(&target_account_id).await,
        "unfollow" => session.client.unfollow_account(&target_account_id).await,
        "mute" => session.client.mute_account(&target_account_id).await,
        "unmute" => session.client.unmute_account(&target_account_id).await,
        "block" => session.client.block_account(&target_account_id).await,
        "unblock" => session.client.unblock_account(&target_account_id).await,
        _ => unreachable!("validated by relationship_operation"),
    }
    .map_err(|error| error.to_string())?;
    Ok(AccountRelationshipSummary {
        following: relationship.following,
        followed_by: relationship.followed_by,
        requested: relationship.requested,
        blocking: relationship.blocking,
        muting: relationship.muting,
    })
}

fn relationship_operation(action: &str) -> Result<RelationshipOperation, String> {
    match action {
        "follow" => Ok(RelationshipOperation::Follow),
        "unfollow" => Ok(RelationshipOperation::Unfollow),
        "mute" => Ok(RelationshipOperation::Mute),
        "unmute" => Ok(RelationshipOperation::Unmute),
        "block" => Ok(RelationshipOperation::Block),
        "unblock" => Ok(RelationshipOperation::Unblock),
        _ => Err(format!("Unsupported account action: {action}")),
    }
}

pub(crate) async fn set_notification_mute(
    state: &RuntimeState,
    request: AccountNotificationMuteRequest,
) -> Result<bool, String> {
    let cached_account = accounts::get_account(
        state.database().reader(),
        &request.account_id,
        &request.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;
    let cached_acct = cached_account
        .as_ref()
        .map(|account| account.acct.as_str())
        .unwrap_or("");
    let cached_display_name = cached_account
        .as_ref()
        .map(|account| account.display_name.as_str())
        .unwrap_or("");
    notification_mutes::set_account_muted(
        state.database().writer(),
        &request.account_id,
        &request.server_domain,
        cached_acct,
        cached_display_name,
        request.muted,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(request.muted)
}

pub(crate) async fn notification_muted_accounts(
    state: &RuntimeState,
) -> Result<Vec<NotificationMutedAccountSummary>, String> {
    let rows = notification_mutes::list_muted_accounts(state.database().reader())
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| NotificationMutedAccountSummary {
            account_id: row.account_id,
            server_domain: row.server_domain,
            acct: row.acct,
            display_name: row.display_name,
            avatar: row.avatar,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

pub(crate) async fn switch_active_account(
    state: &RuntimeState,
    acct: String,
) -> Result<AppSnapshot, String> {
    let mut sessions = state.sessions().write().await;
    if !sessions.sessions().contains_key(&acct) {
        return Err(format!("Account session not found: {acct}"));
    }
    let previous_acct = sessions
        .active_session()
        .map(|session| session.acct.clone());
    settings::set_active_account(state.database().writer(), &acct)
        .await
        .map_err(|error| error.to_string())?;
    if !sessions.set_active(&acct) {
        return Err(format!("Failed to activate account session: {acct}"));
    }
    drop(sessions);
    if let Some(previous_acct) = previous_acct.filter(|previous| previous != &acct) {
        state.media_uploads().cancel_account(&previous_acct).await;
    }

    // Deliberately no restart_streaming here. Active account is only the
    // actor; every signed-in account remains a Unified Timeline source.
    app_snapshot_for_state(state).await
}

pub(crate) async fn logout_account(
    state: &RuntimeState,
    acct: String,
) -> Result<AppSnapshot, String> {
    state.media_uploads().cancel_account(&acct).await;
    let session = state.sessions().read().await.sessions().get(&acct).cloned();
    if let Some(session) = session {
        timeline_service::cancel_pending_quote_resolution(session.client.domain(), &acct);
        session.client.invalidate_auth_generation().await;
    }
    let fallback_acct = state
        .credentials()
        .remove_account_and_reassign(state.database().writer(), &acct)
        .await
        .map_err(|error| error.to_string())?;
    let mut sessions = state.sessions().write().await;
    sessions.remove_session(&acct);
    if let Some(fallback_acct) = fallback_acct.as_deref() {
        sessions.set_active(fallback_acct);
    }
    drop(sessions);

    // Logout removes a source account, so the Unified stream set must be
    // rebuilt. This is distinct from merely switching the acting account.
    restart_streaming(state).await;
    app_snapshot_for_state(state).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn active_account_contract_does_not_name_a_timeline_source() {
        let source = include_str!("account.rs");
        let switch_body = source
            .split("pub(crate) async fn switch_active_account")
            .nth(1)
            .and_then(|tail| tail.split("pub(crate) async fn logout_account").next())
            .expect("switch active account body");

        assert!(!switch_body.contains("restart_streaming("));
        assert!(!switch_body.contains("load_timeline"));
        assert!(!switch_body.contains("account_acct:"));
    }
}
