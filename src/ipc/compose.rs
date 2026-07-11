// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::desktop;
use crate::application::desktop::*;
use crate::ipc::error::AppError;
use crate::mastodon::types::status::MediaAttachment;
use tauri::State;

#[tauri::command]
pub(crate) async fn post_status(
    state: State<'_, RuntimeState>,
    request: PostRequest,
) -> Result<TimelineStatus, AppError> {
    desktop::post_status_impl(state, request).await
}

#[tauri::command]
pub(crate) async fn begin_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: BeginMediaUploadRequest,
) -> Result<BeginMediaUploadResponse, AppError> {
    desktop::begin_compose_media_upload_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("begin_compose_media_upload", error))
}

#[tauri::command]
pub(crate) async fn append_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: AppendMediaUploadRequest,
) -> Result<MediaUploadProgressResponse, AppError> {
    desktop::append_compose_media_upload_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("append_compose_media_upload", error))
}

#[tauri::command]
pub(crate) async fn finish_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
) -> Result<MediaAttachment, AppError> {
    desktop::finish_compose_media_upload_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("finish_compose_media_upload", error))
}

#[tauri::command]
pub(crate) async fn cancel_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
) -> Result<(), AppError> {
    desktop::cancel_compose_media_upload_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("cancel_compose_media_upload", error))
}

#[tauri::command]
pub(crate) fn claim_dropped_media_path(
    state: State<'_, RuntimeState>,
    request: ClaimDroppedMediaPathRequest,
) -> Result<ClaimDroppedMediaPathResponse, AppError> {
    desktop::claim_dropped_media_path_impl(state, request)
        .map_err(|error| desktop::command_error("claim_dropped_media_path", error))
}

#[tauri::command]
pub(crate) async fn upload_compose_media_path(
    state: State<'_, RuntimeState>,
    request: UploadMediaPathRequest,
) -> Result<MediaAttachment, AppError> {
    desktop::upload_compose_media_path_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("upload_compose_media_path", error))
}

#[tauri::command]
pub(crate) async fn autocomplete_mentions(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<MentionSuggestionView>, AppError> {
    desktop::autocomplete_mentions_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("autocomplete_mentions", error))
}

#[tauri::command]
pub(crate) async fn autocomplete_hashtags(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<HashtagSuggestionView>, AppError> {
    desktop::autocomplete_hashtags_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("autocomplete_hashtags", error))
}

#[tauri::command]
pub(crate) async fn custom_emojis(
    state: State<'_, RuntimeState>,
    account_acct: String,
) -> Result<Vec<CustomEmojiView>, AppError> {
    desktop::custom_emojis_impl(state, account_acct)
        .await
        .map_err(|error| desktop::command_error("custom_emojis", error))
}

#[tauri::command]
pub(crate) async fn status_action(
    state: State<'_, RuntimeState>,
    request: StatusActionRequest,
) -> Result<TimelineStatus, AppError> {
    desktop::status_action_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("status_action", error))
}

#[tauri::command]
pub(crate) async fn vote_poll(
    state: State<'_, RuntimeState>,
    request: VotePollRequest,
) -> Result<PollView, AppError> {
    desktop::vote_poll_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("vote_poll", error))
}

#[tauri::command]
pub(crate) async fn edit_own_status(
    state: State<'_, RuntimeState>,
    request: EditStatusRequest,
) -> Result<TimelineStatus, AppError> {
    desktop::edit_own_status_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("edit_own_status", error))
}

#[tauri::command]
pub(crate) async fn delete_own_status(
    state: State<'_, RuntimeState>,
    request: DeleteStatusRequest,
) -> Result<(), AppError> {
    desktop::delete_own_status_impl(state, request)
        .await
        .map_err(|error| desktop::command_error("delete_own_status", error))
}
