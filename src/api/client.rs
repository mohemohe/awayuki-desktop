//! `ApiClient` is the unified entry point used by panels, services, and the workspace.
//!
//! It dispatches each call to either `MastodonClient` (Mastodon / Paon) or `MisskeyClient`,
//! returning the same Mastodon-shaped types either way. New backends can be added by
//! introducing a new variant here.

use std::path::Path;

use crate::api::kind::ServerKind;
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
}

impl ApiClient {
    pub fn kind(&self) -> ServerKind {
        match self {
            Self::Mastodon(_) => ServerKind::Mastodon,
            Self::Misskey(_) => ServerKind::Misskey,
        }
    }

    pub fn access_token(&self) -> &str {
        match self {
            Self::Mastodon(c) => c.access_token(),
            Self::Misskey(c) => c.access_token(),
        }
    }

    pub fn domain(&self) -> &str {
        match self {
            Self::Mastodon(c) => c.domain(),
            Self::Misskey(c) => c.domain(),
        }
    }

    pub fn streaming_url(&self) -> &str {
        match self {
            Self::Mastodon(c) => &c.streaming_url,
            Self::Misskey(c) => &c.streaming_url,
        }
    }

    pub async fn verify_credentials(&self) -> Result<Account, MastodonError> {
        match self {
            Self::Mastodon(c) => c.verify_credentials().await,
            Self::Misskey(c) => c.verify_credentials().await,
        }
    }

    pub async fn get_home_timeline(
        &self,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_home_timeline(params).await,
            Self::Misskey(c) => c.get_home_timeline(params).await,
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
        }
    }

    pub async fn get_bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_bookmarks(params).await,
            Self::Misskey(c) => c.get_bookmarks(params).await,
        }
    }

    pub async fn get_status(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_status(id).await,
            Self::Misskey(c) => c.get_status(id).await,
        }
    }

    pub async fn get_status_context(&self, id: &str) -> Result<StatusContext, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_status_context(id).await,
            Self::Misskey(c) => c.get_status_context(id).await,
        }
    }

    pub async fn get_status_source(&self, id: &str) -> Result<StatusSource, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_status_source(id).await,
            Self::Misskey(c) => c.get_status_source(id).await,
        }
    }

    pub async fn create_status(
        &self,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.create_status(params).await,
            Self::Misskey(c) => c.create_status(params).await,
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
        }
    }

    pub async fn delete_status(&self, id: &str) -> Result<(), MastodonError> {
        match self {
            Self::Mastodon(c) => c.delete_status(id).await,
            Self::Misskey(c) => c.delete_status(id).await,
        }
    }

    pub async fn favourite(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.favourite(id).await,
            Self::Misskey(c) => c.favourite(id).await,
        }
    }

    pub async fn unfavourite(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unfavourite(id).await,
            Self::Misskey(c) => c.unfavourite(id).await,
        }
    }

    pub async fn reblog(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.reblog(id).await,
            Self::Misskey(c) => c.reblog(id).await,
        }
    }

    pub async fn unreblog(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unreblog(id).await,
            Self::Misskey(c) => c.unreblog(id).await,
        }
    }

    pub async fn bookmark(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.bookmark(id).await,
            Self::Misskey(c) => c.bookmark(id).await,
        }
    }

    pub async fn unbookmark(&self, id: &str) -> Result<Status, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unbookmark(id).await,
            Self::Misskey(c) => c.unbookmark(id).await,
        }
    }

    pub async fn get_poll(&self, id: &str) -> Result<Poll, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_poll(id).await,
            Self::Misskey(c) => c.get_poll(id).await,
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
        }
    }

    pub async fn get_notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_notifications(params).await,
            Self::Misskey(c) => c.get_notifications(params).await,
        }
    }

    pub async fn get_notification(&self, id: &str) -> Result<Notification, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_notification(id).await,
            Self::Misskey(c) => c.get_notification(id).await,
        }
    }

    pub async fn dismiss_notification(&self, id: &str) -> Result<(), MastodonError> {
        match self {
            Self::Mastodon(c) => c.dismiss_notification(id).await,
            Self::Misskey(c) => c.dismiss_notification(id).await,
        }
    }

    pub async fn get_account(&self, id: &str) -> Result<Account, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_account(id).await,
            Self::Misskey(c) => c.get_account(id).await,
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
        }
    }

    pub async fn get_relationships(
        &self,
        ids: &[&str],
    ) -> Result<Vec<Relationship>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_relationships(ids).await,
            Self::Misskey(c) => c.get_relationships(ids).await,
        }
    }

    pub async fn follow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.follow_account(id).await,
            Self::Misskey(c) => c.follow_account(id).await,
        }
    }

    pub async fn unfollow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unfollow_account(id).await,
            Self::Misskey(c) => c.unfollow_account(id).await,
        }
    }

    pub async fn mute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.mute_account(id).await,
            Self::Misskey(c) => c.mute_account(id).await,
        }
    }

    pub async fn unmute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unmute_account(id).await,
            Self::Misskey(c) => c.unmute_account(id).await,
        }
    }

    pub async fn block_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.block_account(id).await,
            Self::Misskey(c) => c.block_account(id).await,
        }
    }

    pub async fn unblock_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        match self {
            Self::Mastodon(c) => c.unblock_account(id).await,
            Self::Misskey(c) => c.unblock_account(id).await,
        }
    }

    pub async fn get_lists(&self) -> Result<Vec<List>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_lists().await,
            Self::Misskey(c) => c.get_lists().await,
        }
    }

    pub async fn get_custom_emojis(&self) -> Result<Vec<CustomEmoji>, MastodonError> {
        match self {
            Self::Mastodon(c) => c.get_custom_emojis().await,
            Self::Misskey(c) => c.get_custom_emojis().await,
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
        }
    }

    pub async fn upload_media(&self, file_path: &Path) -> Result<MediaAttachment, MastodonError> {
        match self {
            Self::Mastodon(c) => c.upload_media(file_path).await,
            Self::Misskey(c) => c.upload_media(file_path).await,
        }
    }
}
