//! Compose-scoped media and suggestion resources.
//!
//! Media uploads remain bound to the acting account captured at begin. These
//! resources never alter or narrow Unified Timeline sources.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::application::desktop::{
    acting_session, run_cancellable_read, session_for_timeline_request, CustomEmojiView,
    RuntimeState,
};
use crate::application::settings as settings_application;
use crate::db::queries::{accounts, tags};
use crate::ipc::dto::{
    BeginMediaUploadRequest, ClaimDroppedMediaPathRequest, ComposeSuggestionRequest,
    MediaUploadIdRequest, UploadMediaPathRequest,
};
use crate::ipc::error::AppError;
use crate::mastodon::types::status::MediaAttachment;
use crate::state::performance::{PerformanceSettings, SuggestionSource};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BeginMediaUploadResponse {
    pub(crate) upload_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaUploadProgressResponse {
    pub(crate) written: u64,
    pub(crate) total: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimDroppedMediaPathResponse {
    pub(crate) capability: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MentionSuggestionView {
    pub(crate) acct: String,
    pub(crate) display_name: String,
    pub(crate) avatar: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HashtagSuggestionView {
    pub(crate) name: String,
}

pub(crate) async fn begin_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: BeginMediaUploadRequest,
) -> Result<BeginMediaUploadResponse, String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if !session.client.capabilities(1).compose.media_upload {
        return Err("Media upload is not supported by this account".to_string());
    }
    let upload_id = state
        .media_uploads()
        .begin(
            session.acct,
            &request.filename,
            &request.mime_type,
            request.size,
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(BeginMediaUploadResponse { upload_id })
}

pub(crate) async fn append_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    upload_id: String,
    data: Vec<u8>,
) -> Result<MediaUploadProgressResponse, String> {
    let progress = state
        .media_uploads()
        .append(&upload_id, &data)
        .await
        .map_err(|error| error.to_string())?;
    Ok(MediaUploadProgressResponse {
        written: progress.written,
        total: progress.total,
    })
}

pub(crate) async fn finish_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
) -> Result<MediaAttachment, String> {
    let completed = state
        .media_uploads()
        .finish(&request.upload_id)
        .await
        .map_err(|error| error.to_string())?;
    let session = acting_session(&state, &completed.acting_account_acct).await?;
    if !session.client.capabilities(1).compose.media_upload {
        return Err("Media upload is not supported by this account".to_string());
    }
    session
        .client
        .upload_media(&completed.path)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn cancel_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
) -> Result<(), String> {
    state.media_uploads().cancel(&request.upload_id).await;
    Ok(())
}

pub(crate) async fn claim_dropped_media_path_impl(
    state: State<'_, RuntimeState>,
    request: ClaimDroppedMediaPathRequest,
) -> Result<ClaimDroppedMediaPathResponse, String> {
    let capability = state
        .media_uploads()
        .claim_dropped_path(PathBuf::from(request.path).as_path())
        .map_err(|error| error.to_string())?;
    Ok(ClaimDroppedMediaPathResponse { capability })
}

pub(crate) async fn upload_compose_media_path_impl(
    state: State<'_, RuntimeState>,
    request: UploadMediaPathRequest,
) -> Result<MediaAttachment, String> {
    let path = PathBuf::from(request.path);
    let path = state
        .media_uploads()
        .consume_dropped_path(&request.capability, &path)
        .await
        .map_err(|error| error.to_string())?;
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if !session.client.capabilities(1).compose.media_upload {
        return Err("Media upload is not supported by this account".to_string());
    }
    let client = session.client;
    client
        .upload_media(&path)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn autocomplete_mentions_impl(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<MentionSuggestionView>, AppError> {
    let operation_id = request.operation_id.clone();
    let manager = state.timeline_query_manager().clone();
    run_cancellable_read(
        manager,
        "autocomplete_mentions",
        operation_id.as_deref(),
        autocomplete_mentions_inner(state, request),
    )
    .await
}

async fn autocomplete_mentions_inner(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<MentionSuggestionView>, String> {
    let query = normalize_suggestion_query(&request.query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = normalize_suggestion_limit(request.limit);
    let performance =
        settings_application::load_setting::<PerformanceSettings>(state.database(), "performance")
            .await?;

    match performance.mention_source {
        SuggestionSource::Server => {
            let session =
                session_for_timeline_request(&state, request.account_acct.as_deref()).await?;
            let accounts = session
                .client
                .search_accounts(&query, limit)
                .await
                .map_err(|error| error.to_string())?;
            Ok(unique_mention_suggestions(
                accounts
                    .into_iter()
                    .map(|account| MentionSuggestionView {
                        acct: account.acct,
                        display_name: account.display_name,
                        avatar: account.avatar,
                    })
                    .collect(),
            ))
        }
        SuggestionSource::SQLite => {
            let accounts =
                accounts::search_accounts_prefix(state.database().reader(), &query, limit)
                    .await
                    .map_err(|error| error.to_string())?;
            Ok(unique_mention_suggestions(
                accounts
                    .into_iter()
                    .map(|account| MentionSuggestionView {
                        acct: account.acct,
                        display_name: account.display_name,
                        avatar: account.avatar,
                    })
                    .collect(),
            ))
        }
    }
}

pub(crate) async fn autocomplete_hashtags_impl(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<HashtagSuggestionView>, AppError> {
    let operation_id = request.operation_id.clone();
    let manager = state.timeline_query_manager().clone();
    run_cancellable_read(
        manager,
        "autocomplete_hashtags",
        operation_id.as_deref(),
        autocomplete_hashtags_inner(state, request),
    )
    .await
}

async fn autocomplete_hashtags_inner(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<HashtagSuggestionView>, String> {
    let query = normalize_suggestion_query(&request.query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = normalize_suggestion_limit(request.limit);
    let performance =
        settings_application::load_setting::<PerformanceSettings>(state.database(), "performance")
            .await?;

    let names = match performance.hashtag_source {
        SuggestionSource::Server => {
            let session =
                session_for_timeline_request(&state, request.account_acct.as_deref()).await?;
            session
                .client
                .search_hashtags(&query, limit)
                .await
                .map_err(|error| error.to_string())?
                .hashtags
                .into_iter()
                .map(|tag| tag.name)
                .collect()
        }
        SuggestionSource::SQLite => {
            tags::search_tags_prefix(state.database().reader(), &query, limit)
                .await
                .map_err(|error| error.to_string())?
        }
    };

    Ok(unique_hashtag_names(names)
        .into_iter()
        .map(|name| HashtagSuggestionView { name })
        .collect())
}

fn normalize_suggestion_query(query: &str) -> String {
    query
        .trim()
        .trim_start_matches(['@', '#'])
        .chars()
        .take(80)
        .collect()
}

fn normalize_suggestion_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(8).clamp(1, 20)
}

fn unique_mention_suggestions(
    suggestions: Vec<MentionSuggestionView>,
) -> Vec<MentionSuggestionView> {
    let mut seen = HashSet::new();
    suggestions
        .into_iter()
        .filter(|suggestion| {
            let key = normalize_suggestion_identity(&suggestion.acct, '@');
            !key.is_empty() && seen.insert(key)
        })
        .collect()
}

fn unique_hashtag_names(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter(|name| {
            let key = normalize_suggestion_identity(name, '#');
            !key.is_empty() && seen.insert(key)
        })
        .collect()
}

fn normalize_suggestion_identity(value: &str, marker: char) -> String {
    value.trim().trim_start_matches(marker).to_lowercase()
}

pub(crate) async fn custom_emojis_impl(
    state: State<'_, RuntimeState>,
    account_acct: String,
) -> Result<Vec<CustomEmojiView>, String> {
    let session = acting_session(&state, &account_acct).await?;
    let client = session.client;
    let emojis = client
        .get_custom_emojis()
        .await
        .map_err(|error| error.to_string())?;
    Ok(emojis
        .into_iter()
        .filter(|emoji| emoji.visible_in_picker)
        .map(|emoji| CustomEmojiView {
            shortcode: emoji.shortcode,
            url: emoji.url,
            static_url: emoji.static_url,
            category: emoji.category,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_query_is_normalized_and_bounded() {
        assert_eq!(normalize_suggestion_query("  @Awayuki "), "Awayuki");
        assert_eq!(normalize_suggestion_limit(Some(999)), 20);
        assert!(normalize_suggestion_query("#").is_empty());
        assert_eq!(normalize_suggestion_query(&"a".repeat(100)).len(), 80);
    }
}
