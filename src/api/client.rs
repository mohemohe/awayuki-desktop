//! `ApiClient` is the unified entry point used by panels, services, and the workspace.
//!
//! It dispatches each call to either `MastodonClient` (Mastodon / Paon) or `MisskeyClient`,
//! returning the same Mastodon-shaped types either way. New backends can be added by
//! introducing a new variant here.

use std::path::Path;

use crate::api::kind::ServerKind;
use crate::bluesky::client::BlueskyClient;
use crate::bluesky::rate_limit::RateLimitState;
use crate::mastodon::client::{MastodonClient, PaginatedResponse};
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::statuses::{CreateStatusParams, VotePollParams};
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::{Account, CustomEmoji, Relationship};
use crate::mastodon::types::list::List;
use crate::mastodon::types::notification::Notification;
use crate::mastodon::types::search::SearchResult;
use crate::mastodon::types::status::{MediaAttachment, Poll, Status, StatusContext, StatusSource};
use crate::misskey::client::MisskeyClient;

/// Backend-agnostic client. Each method delegates to the active variant.
#[derive(Clone)]
pub enum ApiClient {
    Mastodon(MastodonClient),
    Misskey(MisskeyClient),
    Bluesky(BlueskyClient),
}

impl ApiClient {
    pub fn kind(&self) -> ServerKind {
        match self {
            Self::Mastodon(_) => ServerKind::Mastodon,
            Self::Misskey(_) => ServerKind::Misskey,
            Self::Bluesky(_) => ServerKind::Bluesky,
        }
    }

    /// Snapshot of the current access token (synchronous). For Bluesky this returns
    /// the last cached snapshot; if you need the post-refresh token (e.g. before
    /// persisting to the DB), use [`Self::current_access_token`] instead.
    pub fn access_token(&self) -> String {
        match self {
            Self::Mastodon(c) => c.access_token().to_string(),
            Self::Misskey(c) => c.access_token().to_string(),
            Self::Bluesky(c) => c.cached_access_token(),
        }
    }

    /// Refreshes (Bluesky) and returns the latest access token, suitable for DB save.
    /// For Mastodon/Misskey this is a no-op equivalent to `access_token`.
    pub async fn current_access_token(&self) -> String {
        match self {
            Self::Mastodon(c) => c.access_token().to_string(),
            Self::Misskey(c) => c.access_token().to_string(),
            Self::Bluesky(c) => c
                .refresh_token()
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("Bluesky token refresh failed: {} — saving cached snapshot", e);
                    c.cached_access_token()
                }),
        }
    }

    pub fn domain(&self) -> &str {
        match self {
            Self::Mastodon(c) => c.domain(),
            Self::Misskey(c) => c.domain(),
            Self::Bluesky(c) => c.domain(),
        }
    }

    pub fn streaming_url(&self) -> &str {
        match self {
            Self::Mastodon(c) => &c.streaming_url,
            Self::Misskey(c) => &c.streaming_url,
            Self::Bluesky(c) => &c.streaming_url,
        }
    }

    /// Shared handle to this account's Bluesky rate-limit slot, or `None`
    /// for non-Bluesky variants. Settings → Account polls this on render to
    /// display "remaining / limit" without going through the network.
    pub fn bluesky_rate_limit_state(&self) -> Option<RateLimitState> {
        match self {
            Self::Bluesky(c) => Some(c.rate_limit_state()),
            _ => None,
        }
    }

    /// App password held by a Bluesky session, or `None` for other backends
    /// (and for Bluesky sessions restored from a DB row that predates the
    /// app-password column). Used by the workspace to persist the password
    /// alongside the access token so we can re-authenticate on token loss.
    pub fn bluesky_app_password(&self) -> Option<String> {
        match self {
            Self::Bluesky(c) => c.cached_app_password(),
            _ => None,
        }
    }

    pub async fn verify_credentials(&self) -> Result<Account, MastodonError> {
        match self {
            Self::Mastodon(c) => c.verify_credentials().await,
            Self::Misskey(c) => c.verify_credentials().await,
            Self::Bluesky(c) => c.verify_credentials().await,
        }
    }

    pub async fn get_home_timeline(
        &self,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_home_timeline(params).await,
            Self::Misskey(c) => c.get_home_timeline(params).await,
            Self::Bluesky(c) => c.get_home_timeline(params).await,
        }
    }

    pub async fn get_public_timeline(
        &self,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_public_timeline(local, params).await,
            Self::Misskey(c) => c.get_public_timeline(local, params).await,
            Self::Bluesky(c) => c.get_public_timeline(local, params).await,
        }
    }

    pub async fn get_list_timeline(
        &self,
        list_id: &str,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_list_timeline(list_id, params).await,
            Self::Misskey(c) => c.get_list_timeline(list_id, params).await,
            Self::Bluesky(c) => c.get_list_timeline(list_id, params).await,
        }
    }

    pub async fn get_hashtag_timeline(
        &self,
        tag: &str,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_hashtag_timeline(tag, local, params).await,
            Self::Misskey(c) => c.get_hashtag_timeline(tag, local, params).await,
            Self::Bluesky(c) => c.get_hashtag_timeline(tag, local, params).await,
        }
    }

    pub async fn get_bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_bookmarks(params).await,
            Self::Misskey(c) => c.get_bookmarks(params).await,
            Self::Bluesky(c) => c.get_bookmarks(params).await,
        }
    }

    pub async fn get_status(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_status(id).await,
            Self::Misskey(c) => c.get_status(id).await,
            Self::Bluesky(c) => c.get_status(id).await,
        }
    }

    pub async fn get_status_context(&self, id: &str) -> Result<StatusContext, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_status_context(id).await,
            Self::Misskey(c) => c.get_status_context(id).await,
            Self::Bluesky(c) => c.get_status_context(id).await,
        }
    }

    pub async fn get_status_source(&self, id: &str) -> Result<StatusSource, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_status_source(id).await,
            Self::Misskey(c) => c.get_status_source(id).await,
            Self::Bluesky(c) => c.get_status_source(id).await,
        }
    }

    pub async fn create_status(
        &self,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.create_status(params).await,
            Self::Misskey(c) => c.create_status(params).await,
            Self::Bluesky(c) => c.create_status(params).await,
        }
    }

    pub async fn edit_status(
        &self,
        id: &str,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.edit_status(id, params).await,
            Self::Misskey(c) => c.edit_status(id, params).await,
            Self::Bluesky(c) => c.edit_status(id, params).await,
        }
    }

    pub async fn delete_status(&self, id: &str) -> Result<(), MastodonError> {
        match self {
            Self::Mastodon(c) => c.delete_status(id).await,
            Self::Misskey(c) => c.delete_status(id).await,
            Self::Bluesky(c) => c.delete_status(id).await,
        }
    }

    pub async fn favourite(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.favourite(id).await,
            Self::Misskey(c) => c.favourite(id).await,
            Self::Bluesky(c) => c.favourite(id).await,
        }
    }

    pub async fn unfavourite(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unfavourite(id).await,
            Self::Misskey(c) => c.unfavourite(id).await,
            Self::Bluesky(c) => c.unfavourite(id).await,
        }
    }

    pub async fn reblog(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.reblog(id).await,
            Self::Misskey(c) => c.reblog(id).await,
            Self::Bluesky(c) => c.reblog(id).await,
        }
    }

    pub async fn unreblog(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unreblog(id).await,
            Self::Misskey(c) => c.unreblog(id).await,
            Self::Bluesky(c) => c.unreblog(id).await,
        }
    }

    pub async fn bookmark(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.bookmark(id).await,
            Self::Misskey(c) => c.bookmark(id).await,
            Self::Bluesky(c) => c.bookmark(id).await,
        }
    }

    pub async fn unbookmark(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unbookmark(id).await,
            Self::Misskey(c) => c.unbookmark(id).await,
            Self::Bluesky(c) => c.unbookmark(id).await,
        }
    }

    pub async fn get_poll(&self, id: &str) -> Result<Poll, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_poll(id).await,
            Self::Misskey(c) => c.get_poll(id).await,
            Self::Bluesky(c) => c.get_poll(id).await,
        }
    }

    pub async fn vote_poll(
        &self,
        id: &str,
        params: &VotePollParams,
    ) -> Result<Poll, MastodonError> {
        match self {
            Self::Mastodon(c) => c.vote_poll(id, params).await,
            Self::Misskey(c) => c.vote_poll(id, params).await,
            Self::Bluesky(c) => c.vote_poll(id, params).await,
        }
    }

    pub async fn get_notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_notifications(params).await,
            Self::Misskey(c) => c.get_notifications(params).await,
            Self::Bluesky(c) => c.get_notifications(params).await,
        }
    }

    pub async fn get_notification(&self, id: &str) -> Result<Notification, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_notification(id).await,
            Self::Misskey(c) => c.get_notification(id).await,
            Self::Bluesky(c) => c.get_notification(id).await,
        }
    }

    pub async fn dismiss_notification(&self, id: &str) -> Result<(), MastodonError> {
        match self {
            Self::Mastodon(c) => c.dismiss_notification(id).await,
            Self::Misskey(c) => c.dismiss_notification(id).await,
            Self::Bluesky(c) => c.dismiss_notification(id).await,
        }
    }

    pub async fn get_account(&self, id: &str) -> Result<Account, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_account(id).await,
            Self::Misskey(c) => c.get_account(id).await,
            Self::Bluesky(c) => c.get_account(id).await,
        }
    }

    pub async fn get_account_statuses(
        &self,
        id: &str,
        params: &AccountStatusesParams,
    ) -> Result<Vec<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_account_statuses(id, params).await,
            Self::Misskey(c) => c.get_account_statuses(id, params).await,
            Self::Bluesky(c) => c.get_account_statuses(id, params).await,
        }
    }

    pub async fn get_relationships(
        &self,
        ids: &[&str],
    ) -> Result<Vec<Relationship>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_relationships(ids).await,
            Self::Misskey(c) => c.get_relationships(ids).await,
            Self::Bluesky(c) => c.get_relationships(ids).await,
        }
    }

    pub async fn follow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.follow_account(id).await,
            Self::Misskey(c) => c.follow_account(id).await,
            Self::Bluesky(c) => c.follow_account(id).await,
        }
    }

    pub async fn unfollow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unfollow_account(id).await,
            Self::Misskey(c) => c.unfollow_account(id).await,
            Self::Bluesky(c) => c.unfollow_account(id).await,
        }
    }

    pub async fn mute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.mute_account(id).await,
            Self::Misskey(c) => c.mute_account(id).await,
            Self::Bluesky(c) => c.mute_account(id).await,
        }
    }

    pub async fn unmute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unmute_account(id).await,
            Self::Misskey(c) => c.unmute_account(id).await,
            Self::Bluesky(c) => c.unmute_account(id).await,
        }
    }

    pub async fn block_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.block_account(id).await,
            Self::Misskey(c) => c.block_account(id).await,
            Self::Bluesky(c) => c.block_account(id).await,
        }
    }

    pub async fn unblock_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unblock_account(id).await,
            Self::Misskey(c) => c.unblock_account(id).await,
            Self::Bluesky(c) => c.unblock_account(id).await,
        }
    }

    pub async fn get_lists(&self) -> Result<Vec<List>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_lists().await,
            Self::Misskey(c) => c.get_lists().await,
            Self::Bluesky(c) => c.get_lists().await,
        }
    }

    pub async fn get_custom_emojis(&self) -> Result<Vec<CustomEmoji>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_custom_emojis().await,
            Self::Misskey(c) => c.get_custom_emojis().await,
            Self::Bluesky(c) => c.get_custom_emojis().await,
        }
    }

    pub async fn search_accounts(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Account>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.search_accounts(query, limit).await,
            Self::Misskey(c) => c.search_accounts(query, limit).await,
            Self::Bluesky(c) => c.search_accounts(query, limit).await,
        }
    }

    pub async fn search_hashtags(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<SearchResult, MastodonError> {
        match self {
            Self::Mastodon(c) => c.search_hashtags(query, limit).await,
            Self::Misskey(c) => c.search_hashtags(query, limit).await,
            Self::Bluesky(c) => c.search_hashtags(query, limit).await,
        }
    }

    /// Resolve a remote ActivityPub URI on this account's server. Used in
    /// unified-timeline mode where the active (action-source) account differs
    /// from the account that fetched the post: actions like boost/favourite
    /// need a status id valid on the active account's server.
    pub async fn lookup_status_by_uri(
        &self,
        uri: &str,
    ) -> Result<Option<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.lookup_status_by_uri(uri).await,
            Self::Misskey(c) => c.lookup_status_by_uri(uri).await,
            Self::Bluesky(c) => c.lookup_status_by_uri(uri).await,
        }
    }

    pub async fn upload_media(&self, file_path: &Path) -> Result<MediaAttachment, MastodonError> {
        match self {
            Self::Mastodon(c) => c.upload_media(file_path).await,
            Self::Misskey(c) => c.upload_media(file_path).await,
            Self::Bluesky(c) => c.upload_media(file_path).await,
        }
    }
}
