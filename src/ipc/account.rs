// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::account::{
    self, AccountListSummary, AccountRelationshipSummary, NotificationMutedAccountSummary,
};
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::dto::{
    AccountFollowRequest, AccountListsRequest, AccountNotificationMuteRequest,
    AccountProfileRequest, AccountTimelineRequest,
};
use crate::ipc::error::AppError;
use tauri::ipc::Request as IpcRequest;
use tauri::State;

#[tauri::command]
pub(crate) async fn account_summaries(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<AccountSummary>, AppError> {
    desktop::observe_string_command(
        "account_summaries",
        &ipc_request,
        account::account_summaries(state.inner()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn account_lists(
    state: State<'_, RuntimeState>,
    request: AccountListsRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<AccountListSummary>, AppError> {
    desktop::observe_string_command(
        "account_lists",
        &ipc_request,
        account::account_lists(state.inner(), request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn account_profile(
    state: State<'_, RuntimeState>,
    request: AccountProfileRequest,
) -> Result<AccountProfileSummary, AppError> {
    account::account_profile(state, request).await
}

#[tauri::command]
pub(crate) async fn account_timeline(
    state: State<'_, RuntimeState>,
    request: AccountTimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    account::account_timeline(state, request).await
}

#[tauri::command]
pub(crate) async fn account_follow_action(
    state: State<'_, RuntimeState>,
    request: AccountFollowRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<AccountRelationshipSummary, AppError> {
    let manager = state.mutation_operation_manager().clone();
    desktop::run_cancellable_ipc_mutation(
        manager,
        "account_follow_action",
        &ipc_request,
        account::follow_action(state.inner(), request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn set_account_notification_mute(
    state: State<'_, RuntimeState>,
    request: AccountNotificationMuteRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<bool, AppError> {
    desktop::observe_string_command(
        "set_account_notification_mute",
        &ipc_request,
        account::set_notification_mute(state.inner(), request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn notification_muted_accounts(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<NotificationMutedAccountSummary>, AppError> {
    desktop::observe_string_command(
        "notification_muted_accounts",
        &ipc_request,
        account::notification_muted_accounts(state.inner()),
    )
    .await
}

#[tauri::command]
pub(crate) async fn switch_active_account(
    state: State<'_, RuntimeState>,
    acct: String,
    ipc_request: IpcRequest<'_>,
) -> Result<AppSnapshot, AppError> {
    desktop::observe_string_command(
        "switch_active_account",
        &ipc_request,
        account::switch_active_account(state.inner(), acct),
    )
    .await
}

#[tauri::command]
pub(crate) async fn logout_account(
    state: State<'_, RuntimeState>,
    acct: String,
    ipc_request: IpcRequest<'_>,
) -> Result<AppSnapshot, AppError> {
    desktop::observe_string_command(
        "logout_account",
        &ipc_request,
        account::logout_account(state.inner(), acct),
    )
    .await
}
