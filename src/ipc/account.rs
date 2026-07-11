// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use tauri::State;

#[tauri::command]
pub(crate) async fn account_summaries(
    state: State<'_, RuntimeState>,
) -> Result<Vec<AccountSummary>, AppError> {
    desktop::account_summaries_impl(state)
        .await
        .map_err(|error| desktop::command_error("account_summaries", error))
}

#[tauri::command]
pub(crate) async fn account_lists(
    state: State<'_, RuntimeState>,
    request: AccountListsRequest,
) -> Result<Vec<AccountListSummary>, AppError> {
    desktop::account_lists_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("account_lists", error))
}

#[tauri::command]
pub(crate) async fn account_profile(
    state: State<'_, RuntimeState>,
    request: AccountProfileRequest,
) -> Result<AccountProfileSummary, AppError> {
    desktop::account_profile_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("account_profile", error))
}

#[tauri::command]
pub(crate) async fn account_timeline(
    state: State<'_, RuntimeState>,
    request: AccountTimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    desktop::account_timeline_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("account_timeline", error))
}

#[tauri::command]
pub(crate) async fn account_follow_action(
    state: State<'_, RuntimeState>,
    request: AccountFollowRequest,
) -> Result<AccountRelationshipSummary, AppError> {
    desktop::account_follow_action_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("account_follow_action", error))
}

#[tauri::command]
pub(crate) async fn set_account_notification_mute(
    state: State<'_, RuntimeState>,
    request: AccountNotificationMuteRequest,
) -> Result<bool, AppError> {
    desktop::set_account_notification_mute_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("set_account_notification_mute", error))
}

#[tauri::command]
pub(crate) async fn notification_muted_accounts(
    state: State<'_, RuntimeState>,
) -> Result<Vec<NotificationMutedAccountSummary>, AppError> {
    desktop::notification_muted_accounts_impl(state)
        .await
        .map_err(|error| desktop::command_error("notification_muted_accounts", error))
}

#[tauri::command]
pub(crate) async fn switch_active_account(
    state: State<'_, RuntimeState>,
    acct: String,
) -> Result<AppSnapshot, AppError> {
    desktop::switch_active_account_impl(state, acct)
        .await
        .map_err(|error| desktop::command_error("switch_active_account", error))
}

#[tauri::command]
pub(crate) async fn logout_account(
    state: State<'_, RuntimeState>,
    acct: String,
) -> Result<AppSnapshot, AppError> {
    desktop::logout_account_impl(state, acct)
        .await
        .map_err(|error| desktop::command_error("logout_account", error))
}
