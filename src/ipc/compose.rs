// Thin Tauri IPC handlers generated during the ARCH-01 boundary split.
use crate::application::compose::{
    self, BeginMediaUploadResponse, ClaimDroppedMediaPathResponse, HashtagSuggestionView,
    MediaUploadProgressResponse, MentionSuggestionView,
};
use crate::application::desktop;
use crate::application::desktop::*;
use crate::application::status;
use crate::ipc::dto::{
    BeginMediaUploadRequest, ClaimDroppedMediaPathRequest, ComposeOutboxItemRequest,
    ComposeSuggestionRequest, DeleteStatusRequest, EditStatusRequest, MediaUploadIdRequest,
    PostRequest, StatusActionRequest, UploadMediaPathRequest, VotePollRequest,
};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::mastodon::types::status::MediaAttachment;
use crate::services::compose_outbox::{self, ComposeOutboxItemView};
use tauri::ipc::{InvokeBody, Request as IpcRequest};
use tauri::State;

#[tauri::command]
pub(crate) async fn post_status(
    state: State<'_, RuntimeState>,
    request: PostRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<TimelineStatus, AppError> {
    let operation_id = desktop::ipc_operation_id(&ipc_request);
    let manager = state.mutation_operation_manager().clone();
    desktop::run_cancellable_app_mutation(
        manager,
        operation_id.as_deref(),
        status::post_status(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn enqueue_post_status(
    state: State<'_, RuntimeState>,
    request: PostRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<ComposeOutboxItemView, AppError> {
    let operation_id =
        desktop::ipc_operation_id(&ipc_request).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    compose_outbox::enqueue_post(state.inner(), request, operation_id).await
}

#[tauri::command]
pub(crate) async fn enqueue_edit_status(
    state: State<'_, RuntimeState>,
    request: EditStatusRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<ComposeOutboxItemView, AppError> {
    let operation_id =
        desktop::ipc_operation_id(&ipc_request).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    compose_outbox::enqueue_edit(state.inner(), request, operation_id).await
}

#[tauri::command]
pub(crate) async fn compose_outbox_items(
    state: State<'_, RuntimeState>,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<ComposeOutboxItemView>, AppError> {
    let operation_id =
        desktop::ipc_operation_id(&ipc_request).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    compose_outbox::list(state.inner(), &operation_id).await
}

#[tauri::command]
pub(crate) async fn retry_compose_outbox_item(
    state: State<'_, RuntimeState>,
    request: ComposeOutboxItemRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<ComposeOutboxItemView, AppError> {
    let operation_id =
        desktop::ipc_operation_id(&ipc_request).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    compose_outbox::retry(state.inner(), &request.id, &operation_id).await
}

#[tauri::command]
pub(crate) async fn cancel_compose_outbox_item(
    state: State<'_, RuntimeState>,
    request: ComposeOutboxItemRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<ComposeOutboxItemView, AppError> {
    let operation_id =
        desktop::ipc_operation_id(&ipc_request).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    compose_outbox::cancel(state.inner(), &request.id, &operation_id).await
}

#[tauri::command]
pub(crate) async fn begin_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: BeginMediaUploadRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<BeginMediaUploadResponse, AppError> {
    desktop::observe_string_command(
        "begin_compose_media_upload",
        &ipc_request,
        compose::begin_compose_media_upload_impl(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn append_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: IpcRequest<'_>,
) -> Result<MediaUploadProgressResponse, AppError> {
    let request_id = request
        .headers()
        .get("x-awayuki-operation-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let upload_id = request
        .headers()
        .get("x-awayuki-upload-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::from_code(
                AppErrorCode::Validation,
                "upload ID header is required",
                &request_id,
            )
        })?
        .to_string();
    let data = match request.body() {
        InvokeBody::Raw(data) => data.clone(),
        InvokeBody::Json(_) => {
            return Err(AppError::from_code(
                AppErrorCode::Validation,
                "raw media chunk body is required",
                request_id,
            ));
        }
    };
    compose::append_compose_media_upload_impl(state, upload_id, data)
        .await
        .map_err(|error| AppError::from_source(error, request_id))
}

#[tauri::command]
pub(crate) async fn finish_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<MediaAttachment, AppError> {
    desktop::observe_string_command(
        "finish_compose_media_upload",
        &ipc_request,
        compose::finish_compose_media_upload_impl(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn cancel_compose_media_upload(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    desktop::observe_string_command(
        "cancel_compose_media_upload",
        &ipc_request,
        compose::cancel_compose_media_upload_impl(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn claim_dropped_media_path(
    state: State<'_, RuntimeState>,
    request: ClaimDroppedMediaPathRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<ClaimDroppedMediaPathResponse, AppError> {
    desktop::observe_string_command(
        "claim_dropped_media_path",
        &ipc_request,
        compose::claim_dropped_media_path_impl(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn upload_compose_media_path(
    state: State<'_, RuntimeState>,
    request: UploadMediaPathRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<MediaAttachment, AppError> {
    desktop::observe_string_command(
        "upload_compose_media_path",
        &ipc_request,
        compose::upload_compose_media_path_impl(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn autocomplete_mentions(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<MentionSuggestionView>, AppError> {
    compose::autocomplete_mentions_impl(state, request).await
}

#[tauri::command]
pub(crate) async fn autocomplete_hashtags(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<HashtagSuggestionView>, AppError> {
    compose::autocomplete_hashtags_impl(state, request).await
}

#[tauri::command]
pub(crate) async fn custom_emojis(
    state: State<'_, RuntimeState>,
    account_acct: String,
    ipc_request: IpcRequest<'_>,
) -> Result<Vec<CustomEmojiView>, AppError> {
    desktop::observe_string_command(
        "custom_emojis",
        &ipc_request,
        compose::custom_emojis_impl(state, account_acct),
    )
    .await
}

#[tauri::command]
pub(crate) async fn status_action(
    state: State<'_, RuntimeState>,
    request: StatusActionRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<TimelineStatus, AppError> {
    let manager = state.mutation_operation_manager().clone();
    desktop::run_cancellable_ipc_mutation(
        manager,
        "status_action",
        &ipc_request,
        status::status_action(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn vote_poll(
    state: State<'_, RuntimeState>,
    request: VotePollRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<PollView, AppError> {
    let manager = state.mutation_operation_manager().clone();
    desktop::run_cancellable_ipc_mutation(
        manager,
        "vote_poll",
        &ipc_request,
        status::vote_poll(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn edit_own_status(
    state: State<'_, RuntimeState>,
    request: EditStatusRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<TimelineStatus, AppError> {
    let manager = state.mutation_operation_manager().clone();
    desktop::run_cancellable_ipc_mutation(
        manager,
        "edit_own_status",
        &ipc_request,
        status::edit_own_status(state, request),
    )
    .await
}

#[tauri::command]
pub(crate) async fn delete_own_status(
    state: State<'_, RuntimeState>,
    request: DeleteStatusRequest,
    ipc_request: IpcRequest<'_>,
) -> Result<(), AppError> {
    let manager = state.mutation_operation_manager().clone();
    desktop::run_cancellable_ipc_mutation(
        manager,
        "delete_own_status",
        &ipc_request,
        status::delete_own_status(state, request),
    )
    .await
}
