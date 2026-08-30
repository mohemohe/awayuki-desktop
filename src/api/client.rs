//! Backend-agnostic facade over protocol capability ports.
//!
//! The facade preserves the application-facing API while provider dispatch is
//! implemented once by each adapter in `api::ports`. Adding a protocol no
//! longer requires editing a match expression for every method here.

use std::{future::Future, path::Path, sync::Arc};

use crate::api::kind::ServerKind;
use crate::api::ports::{
    BlueskyAdapter, MastodonAdapter, MisskeyAdapter, ProtocolAdapter, ServerMetadata,
};
use crate::api::retry;
use crate::bluesky::client::{BlueskyClient, BlueskyCredentialSink};
use crate::bluesky::rate_limit::RateLimitState;
use crate::domain::adapter_error::AdapterError;
use crate::domain::capability::{
    ComposeCapabilities, RelationshipCapabilities, SessionCapabilities, StatusCapabilities,
    TimelineCapabilities,
};
use crate::domain::identity::FederationProtocol;
use crate::domain::protocol::{
    Account, AccountStatusesQuery as AccountStatusesParams, CustomEmoji, List, MediaAttachment,
    Notification, NotificationQuery as NotificationParams, Page as PaginatedResponse, Poll,
    PollVote as VotePollParams, Relationship, SearchResult, Status, StatusContext,
    StatusDraft as CreateStatusParams, TimelineQuery as TimelineParams,
};
use crate::mastodon::client::MastodonClient;
use crate::misskey::client::MisskeyClient;

#[derive(Clone)]
pub struct ApiClient {
    adapter: Arc<dyn ProtocolAdapter>,
}

impl ApiClient {
    async fn retry_read<T, F, Fut>(
        &self,
        operation: &'static str,
        request: F,
    ) -> Result<T, AdapterError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, AdapterError>>,
    {
        retry::idempotent(self.domain(), operation, request).await
    }

    pub fn mastodon_with_kind(client: MastodonClient, kind: ServerKind) -> Self {
        Self {
            adapter: Arc::new(MastodonAdapter::new(client, kind)),
        }
    }

    pub fn misskey(client: MisskeyClient) -> Self {
        Self {
            adapter: Arc::new(MisskeyAdapter::new(client)),
        }
    }

    pub fn bluesky(client: BlueskyClient) -> Self {
        Self {
            adapter: Arc::new(BlueskyAdapter::new(client)),
        }
    }

    pub fn kind(&self) -> ServerKind {
        self.adapter.kind()
    }

    pub fn capabilities(&self, max_characters: u32) -> SessionCapabilities {
        Self::capabilities_for_kind(self.kind(), max_characters)
    }

    pub fn capabilities_for_kind(kind: ServerKind, max_characters: u32) -> SessionCapabilities {
        match kind {
            ServerKind::Mastodon | ServerKind::Paon | ServerKind::Misskey => SessionCapabilities {
                protocol: FederationProtocol::ActivityPub,
                timelines: TimelineCapabilities {
                    home: true,
                    public: true,
                    local: true,
                    lists: true,
                    feeds: false,
                    hashtags: true,
                    notifications: true,
                    bookmarks: true,
                    favourites: true,
                },
                status: StatusCapabilities {
                    favourite: true,
                    reblog: true,
                    bookmark: true,
                    vote: true,
                    edit: true,
                    delete: true,
                },
                relationship: RelationshipCapabilities {
                    follow: true,
                    mute: true,
                    block: true,
                },
                compose: ComposeCapabilities {
                    media_upload: true,
                    poll: true,
                    quote: true,
                    max_media_attachments: 4,
                    max_characters,
                },
                streaming: true,
            },
            ServerKind::Bluesky => SessionCapabilities {
                protocol: FederationProtocol::AtProto,
                timelines: TimelineCapabilities {
                    home: true,
                    public: false,
                    local: false,
                    lists: true,
                    feeds: true,
                    hashtags: true,
                    notifications: true,
                    bookmarks: true,
                    favourites: false,
                },
                status: StatusCapabilities {
                    favourite: true,
                    reblog: true,
                    bookmark: true,
                    vote: false,
                    edit: true,
                    delete: true,
                },
                relationship: RelationshipCapabilities {
                    follow: true,
                    mute: true,
                    block: true,
                },
                compose: ComposeCapabilities {
                    media_upload: false,
                    poll: false,
                    quote: true,
                    max_media_attachments: 0,
                    max_characters,
                },
                streaming: true,
            },
        }
    }

    pub fn access_token(&self) -> String {
        self.adapter.access_token()
    }

    pub async fn current_access_token(&self) -> Result<String, AdapterError> {
        self.adapter.current_access_token().await
    }

    pub fn set_bluesky_credential_sink(&self, sink: Arc<dyn BlueskyCredentialSink>) {
        self.adapter.set_bluesky_credential_sink(sink);
    }

    pub async fn invalidate_auth_generation(&self) {
        self.adapter.invalidate_auth_generation().await;
    }

    pub fn domain(&self) -> &str {
        self.adapter.domain()
    }

    pub fn streaming_url(&self) -> &str {
        self.adapter.streaming_url()
    }

    pub fn bluesky_rate_limit_state(&self) -> Option<RateLimitState> {
        self.adapter.bluesky_rate_limit_state()
    }

    pub fn bluesky_app_password(&self) -> Option<String> {
        self.adapter.bluesky_app_password()
    }

    pub fn bluesky_polling_client(&self) -> Option<BlueskyClient> {
        self.adapter.bluesky_polling_client()
    }

    pub async fn server_metadata(
        &self,
        stored_kind: ServerKind,
    ) -> Result<ServerMetadata, AdapterError> {
        self.retry_read("server_metadata", || {
            self.adapter.server_metadata(stored_kind)
        })
        .await
    }

    pub async fn verify_credentials(&self) -> Result<Account, AdapterError> {
        self.retry_read("verify_credentials", || self.adapter.verify_credentials())
            .await
    }

    pub async fn get_home_timeline(
        &self,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, AdapterError> {
        self.retry_read("home", || self.adapter.home(params)).await
    }

    pub async fn get_public_timeline(
        &self,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, AdapterError> {
        self.retry_read("public", || self.adapter.public(local, params))
            .await
    }

    pub async fn get_list_timeline(
        &self,
        list_id: &str,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, AdapterError> {
        self.retry_read("list", || self.adapter.list(list_id, params))
            .await
    }

    pub async fn get_feed_timeline(
        &self,
        feed_id: &str,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, AdapterError> {
        self.retry_read("feed", || self.adapter.feed(feed_id, params))
            .await
    }

    pub async fn get_hashtag_timeline(
        &self,
        tag: &str,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, AdapterError> {
        self.retry_read("hashtag", || self.adapter.hashtag(tag, local, params))
            .await
    }

    pub async fn get_bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, AdapterError> {
        self.retry_read("bookmarks", || self.adapter.bookmarks(params))
            .await
    }

    pub async fn get_favourites(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, AdapterError> {
        self.retry_read("favourites", || self.adapter.favourites(params))
            .await
    }

    pub async fn get_status(&self, id: &str) -> Result<Status, AdapterError> {
        self.retry_read("status", || self.adapter.status(id)).await
    }

    pub async fn get_status_context(&self, id: &str) -> Result<StatusContext, AdapterError> {
        self.retry_read("status_context", || self.adapter.status_context(id))
            .await
    }

    pub async fn create_status(&self, params: &CreateStatusParams) -> Result<Status, AdapterError> {
        self.adapter.create(params).await
    }

    pub async fn edit_status(
        &self,
        id: &str,
        params: &CreateStatusParams,
    ) -> Result<Status, AdapterError> {
        self.adapter.edit(id, params).await
    }

    pub async fn delete_status(&self, id: &str) -> Result<(), AdapterError> {
        self.adapter.delete(id).await
    }

    pub async fn favourite(&self, id: &str) -> Result<Status, AdapterError> {
        self.adapter.favourite(id).await
    }

    pub async fn unfavourite(&self, id: &str) -> Result<Status, AdapterError> {
        self.adapter.unfavourite(id).await
    }

    pub async fn reblog(&self, id: &str) -> Result<Status, AdapterError> {
        self.adapter.reblog(id).await
    }

    pub async fn unreblog(&self, id: &str) -> Result<Status, AdapterError> {
        self.adapter.unreblog(id).await
    }

    pub async fn bookmark(&self, id: &str) -> Result<Status, AdapterError> {
        self.adapter.bookmark(id).await
    }

    pub async fn unbookmark(&self, id: &str) -> Result<Status, AdapterError> {
        self.adapter.unbookmark(id).await
    }

    pub async fn vote_poll(&self, id: &str, params: &VotePollParams) -> Result<Poll, AdapterError> {
        self.adapter.vote_poll(id, params).await
    }

    pub async fn get_notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, AdapterError> {
        self.retry_read("notifications", || self.adapter.notifications(params))
            .await
    }

    pub async fn get_account(&self, id: &str) -> Result<Account, AdapterError> {
        self.retry_read("account", || self.adapter.account(id))
            .await
    }

    pub async fn get_account_statuses(
        &self,
        id: &str,
        params: &AccountStatusesParams,
    ) -> Result<Vec<Status>, AdapterError> {
        Ok(self.get_account_statuses_page(id, params).await?.data)
    }

    pub async fn get_account_statuses_page(
        &self,
        id: &str,
        params: &AccountStatusesParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, AdapterError> {
        self.retry_read("account_statuses", || {
            self.adapter.account_statuses(id, params)
        })
        .await
    }

    pub async fn get_relationships(&self, ids: &[&str]) -> Result<Vec<Relationship>, AdapterError> {
        self.retry_read("relationships", || self.adapter.relationships(ids))
            .await
    }

    pub async fn follow_account(&self, id: &str) -> Result<Relationship, AdapterError> {
        self.adapter.follow(id).await
    }

    pub async fn unfollow_account(&self, id: &str) -> Result<Relationship, AdapterError> {
        self.adapter.unfollow(id).await
    }

    pub async fn mute_account(&self, id: &str) -> Result<Relationship, AdapterError> {
        self.adapter.mute(id).await
    }

    pub async fn unmute_account(&self, id: &str) -> Result<Relationship, AdapterError> {
        self.adapter.unmute(id).await
    }

    pub async fn block_account(&self, id: &str) -> Result<Relationship, AdapterError> {
        self.adapter.block(id).await
    }

    pub async fn unblock_account(&self, id: &str) -> Result<Relationship, AdapterError> {
        self.adapter.unblock(id).await
    }

    pub async fn get_lists(&self) -> Result<Vec<List>, AdapterError> {
        self.retry_read("lists", || self.adapter.lists()).await
    }

    pub async fn get_saved_feeds(&self) -> Result<Vec<List>, AdapterError> {
        self.retry_read("feeds", || self.adapter.feeds()).await
    }

    pub async fn get_custom_emojis(&self) -> Result<Vec<CustomEmoji>, AdapterError> {
        self.retry_read("custom_emojis", || self.adapter.custom_emojis())
            .await
    }

    pub async fn search_accounts(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Account>, AdapterError> {
        self.retry_read("search_accounts", || {
            self.adapter.search_accounts(query, limit)
        })
        .await
    }

    pub async fn search_hashtags(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<SearchResult, AdapterError> {
        self.retry_read("search_hashtags", || {
            self.adapter.search_hashtags(query, limit)
        })
        .await
    }

    pub async fn lookup_status_by_uri(&self, uri: &str) -> Result<Option<Status>, AdapterError> {
        // Quote hydration owns a wider bounded retry/negative-cache policy;
        // nesting the generic read retry here would multiply its attempts.
        self.adapter.lookup_status_by_uri(uri).await
    }

    pub async fn upload_media(&self, file_path: &Path) -> Result<MediaAttachment, AdapterError> {
        self.adapter.upload(file_path).await
    }
}
