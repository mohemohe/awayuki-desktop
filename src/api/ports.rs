//! Protocol capability ports.
//!
//! Each provider implements these small interfaces once. `ApiClient` stores a
//! trait object, so adding a protocol implements ports in its own adapter
//! without extending a match expression in every application operation.

use std::{future::Future, path::Path, sync::Arc};

use async_trait::async_trait;

use crate::api::kind::ServerKind;
use crate::bluesky::client::{BlueskyClient, BlueskyCredentialSink};
use crate::bluesky::rate_limit::RateLimitState;
use crate::domain::adapter_error::{AdapterError, AdapterErrorCode, AdapterResult, AdapterSource};
use crate::domain::protocol::{
    Account, AccountStatusesQuery, CustomEmoji, List, MediaAttachment, Notification,
    NotificationQuery, Page, Poll, PollVote, Relationship, SearchResult, Status, StatusContext,
    StatusDraft, TimelineQuery,
};
use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError as LegacyProtocolError;
use crate::mastodon::types::instance::Instance;
use crate::misskey::client::MisskeyClient;

const MASTODON_CHARACTER_LIMIT: i32 = 500;
const MISSKEY_CHARACTER_LIMIT: i32 = 3_000;
const BLUESKY_CHARACTER_LIMIT: i32 = 300;

// Adapter implementation aliases keep provider method signatures readable;
// public port traits above expose only domain names and AdapterError.
type PortError = AdapterError;
type TimelineParams = TimelineQuery;
type AccountStatusesParams = AccountStatusesQuery;
type NotificationParams = NotificationQuery;
type CreateStatusParams = StatusDraft;
type VotePollParams = PollVote;
type PaginatedResponse<T> = Page<T>;

fn adapt_error(kind: ServerKind, error: LegacyProtocolError) -> AdapterError {
    let source = match kind {
        ServerKind::Mastodon | ServerKind::Paon => AdapterSource::ActivityPub,
        ServerKind::Misskey => AdapterSource::Misskey,
        ServerKind::Bluesky => AdapterSource::AtProto,
    };
    let (code, retry_after) = match &error {
        LegacyProtocolError::Http(error) if error.is_timeout() => (AdapterErrorCode::Timeout, None),
        LegacyProtocolError::Http(_) => (AdapterErrorCode::Transport, None),
        LegacyProtocolError::Api { status: 401, .. } | LegacyProtocolError::Unauthorized => {
            (AdapterErrorCode::Unauthorized, None)
        }
        LegacyProtocolError::Api { status: 429, .. } => (AdapterErrorCode::RateLimited, None),
        LegacyProtocolError::RateLimited { retry_after } => {
            (AdapterErrorCode::RateLimited, *retry_after)
        }
        LegacyProtocolError::IncompatibleInstance(_) => (AdapterErrorCode::Unsupported, None),
        LegacyProtocolError::Json(_) => (AdapterErrorCode::InvalidResponse, None),
        LegacyProtocolError::Url(_)
        | LegacyProtocolError::Api { .. }
        | LegacyProtocolError::Other(_) => (AdapterErrorCode::Internal, None),
    };
    AdapterError::new(code, source, retry_after, error)
}

fn adapt_result<T>(kind: ServerKind, result: Result<T, LegacyProtocolError>) -> AdapterResult<T> {
    result.map_err(|error| adapt_error(kind, error))
}

#[derive(Debug, Clone)]
pub struct ServerMetadata {
    pub streaming_url: String,
    pub version: Option<String>,
    pub max_characters: i32,
    pub instance_json: Option<String>,
    pub server_kind: ServerKind,
}

#[async_trait]
pub trait SessionPort: Send + Sync {
    fn kind(&self) -> ServerKind;
    fn access_token(&self) -> String;
    async fn current_access_token(&self) -> AdapterResult<String>;
    fn domain(&self) -> &str;
    fn streaming_url(&self) -> &str;
    fn set_bluesky_credential_sink(&self, _sink: Arc<dyn BlueskyCredentialSink>) {}
    async fn invalidate_auth_generation(&self) {}
    fn bluesky_rate_limit_state(&self) -> Option<RateLimitState> {
        None
    }
    fn bluesky_app_password(&self) -> Option<String> {
        None
    }
    fn bluesky_polling_client(&self) -> Option<BlueskyClient> {
        None
    }
    async fn server_metadata(&self, stored_kind: ServerKind) -> AdapterResult<ServerMetadata>;
}

#[async_trait]
pub trait TimelineReader: Send + Sync {
    async fn verify_credentials(&self) -> AdapterResult<Account>;
    async fn home(&self, params: &TimelineQuery) -> AdapterResult<Vec<Status>>;
    async fn public(&self, local: bool, params: &TimelineQuery) -> AdapterResult<Vec<Status>>;
    async fn list(&self, list_id: &str, params: &TimelineQuery) -> AdapterResult<Vec<Status>>;
    async fn feed(&self, feed_id: &str, params: &TimelineQuery) -> AdapterResult<Vec<Status>>;
    async fn hashtag(
        &self,
        tag: &str,
        local: bool,
        params: &TimelineQuery,
    ) -> AdapterResult<Vec<Status>>;
    async fn bookmarks(&self, params: &TimelineQuery) -> AdapterResult<Page<Vec<Status>>>;
    async fn favourites(&self, params: &TimelineQuery) -> AdapterResult<Page<Vec<Status>>>;
    async fn status(&self, id: &str) -> AdapterResult<Status>;
    async fn status_context(&self, id: &str) -> AdapterResult<StatusContext>;
    async fn notifications(&self, params: &NotificationQuery) -> AdapterResult<Vec<Notification>>;
}

#[async_trait]
pub trait StatusMutator: Send + Sync {
    async fn create(&self, params: &StatusDraft) -> AdapterResult<Status>;
    async fn edit(&self, id: &str, params: &StatusDraft) -> AdapterResult<Status>;
    async fn delete(&self, id: &str) -> AdapterResult<()>;
    async fn favourite(&self, id: &str) -> AdapterResult<Status>;
    async fn unfavourite(&self, id: &str) -> AdapterResult<Status>;
    async fn reblog(&self, id: &str) -> AdapterResult<Status>;
    async fn unreblog(&self, id: &str) -> AdapterResult<Status>;
    async fn bookmark(&self, id: &str) -> AdapterResult<Status>;
    async fn unbookmark(&self, id: &str) -> AdapterResult<Status>;
    async fn vote_poll(&self, id: &str, params: &PollVote) -> AdapterResult<Poll>;
    async fn lookup_status_by_uri(&self, uri: &str) -> AdapterResult<Option<Status>>;
}

#[async_trait]
pub trait RelationshipManager: Send + Sync {
    async fn account(&self, id: &str) -> AdapterResult<Account>;
    async fn account_statuses(
        &self,
        id: &str,
        params: &AccountStatusesQuery,
    ) -> AdapterResult<Page<Vec<Status>>>;
    async fn relationships(&self, ids: &[&str]) -> AdapterResult<Vec<Relationship>>;
    async fn follow(&self, id: &str) -> AdapterResult<Relationship>;
    async fn unfollow(&self, id: &str) -> AdapterResult<Relationship>;
    async fn mute(&self, id: &str) -> AdapterResult<Relationship>;
    async fn unmute(&self, id: &str) -> AdapterResult<Relationship>;
    async fn block(&self, id: &str) -> AdapterResult<Relationship>;
    async fn unblock(&self, id: &str) -> AdapterResult<Relationship>;
}

#[async_trait]
pub trait DiscoveryReader: Send + Sync {
    async fn lists(&self) -> AdapterResult<Vec<List>>;
    async fn feeds(&self) -> AdapterResult<Vec<List>>;
    async fn custom_emojis(&self) -> AdapterResult<Vec<CustomEmoji>>;
    async fn search_accounts(&self, query: &str, limit: u32) -> AdapterResult<Vec<Account>>;
    async fn search_hashtags(&self, query: &str, limit: u32) -> AdapterResult<SearchResult>;
}

#[async_trait]
pub trait MediaUploader: Send + Sync {
    async fn upload(&self, file_path: &Path) -> AdapterResult<MediaAttachment>;
}

pub trait ProtocolAdapter:
    SessionPort + TimelineReader + StatusMutator + RelationshipManager + DiscoveryReader + MediaUploader
{
}

impl<T> ProtocolAdapter for T where
    T: SessionPort
        + TimelineReader
        + StatusMutator
        + RelationshipManager
        + DiscoveryReader
        + MediaUploader
{
}

pub struct MastodonAdapter {
    client: MastodonClient,
    kind: ServerKind,
}

impl MastodonAdapter {
    pub fn new(client: MastodonClient, kind: ServerKind) -> Self {
        debug_assert!(matches!(kind, ServerKind::Mastodon | ServerKind::Paon));
        Self { client, kind }
    }
}

pub struct MisskeyAdapter {
    client: MisskeyClient,
}

impl MisskeyAdapter {
    pub fn new(client: MisskeyClient) -> Self {
        Self { client }
    }
}

pub struct BlueskyAdapter {
    client: BlueskyClient,
}

impl BlueskyAdapter {
    pub fn new(client: BlueskyClient) -> Self {
        Self { client }
    }

    async fn with_auth_retry<T, F, Fut>(
        &self,
        label: &str,
        operation: F,
    ) -> Result<T, LegacyProtocolError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, LegacyProtocolError>>,
    {
        match operation().await {
            Ok(value) => {
                self.client.refresh_token().await?;
                Ok(value)
            }
            Err(error) if BlueskyClient::is_auth_error(&error) => {
                tracing::warn!(
                    "Bluesky {} returned unauthorized; attempting token refresh/app-password fallback",
                    label
                );
                self.client.recover_authentication().await?;
                let value = operation().await?;
                self.client.refresh_token().await?;
                Ok(value)
            }
            Err(error) => {
                self.client.refresh_token().await?;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl SessionPort for MastodonAdapter {
    fn kind(&self) -> ServerKind {
        self.kind
    }
    fn access_token(&self) -> String {
        self.client.access_token().to_string()
    }
    async fn current_access_token(&self) -> AdapterResult<String> {
        Ok(self.client.access_token().to_string())
    }
    fn domain(&self) -> &str {
        self.client.domain()
    }
    fn streaming_url(&self) -> &str {
        &self.client.streaming_url
    }
    async fn server_metadata(&self, stored_kind: ServerKind) -> AdapterResult<ServerMetadata> {
        let instance: Instance = match self.client.get("/api/v2/instance").await {
            Ok(instance) => instance,
            Err(LegacyProtocolError::Api { status: 404, .. }) => {
                adapt_result(self.kind, self.client.get("/api/v1/instance").await)?
            }
            Err(error) => return Err(adapt_error(self.kind, error)),
        };
        Ok(ServerMetadata {
            streaming_url: instance
                .streaming_url()
                .unwrap_or(self.client.streaming_url.as_str())
                .to_string(),
            version: Some(instance.version.clone()),
            max_characters: normalize_limit(instance.max_characters(), MASTODON_CHARACTER_LIMIT),
            instance_json: serde_json::to_string(&instance).ok(),
            server_kind: stored_kind,
        })
    }
}

#[async_trait]
impl SessionPort for MisskeyAdapter {
    fn kind(&self) -> ServerKind {
        ServerKind::Misskey
    }
    fn access_token(&self) -> String {
        self.client.access_token().to_string()
    }
    async fn current_access_token(&self) -> AdapterResult<String> {
        Ok(self.client.access_token().to_string())
    }
    fn domain(&self) -> &str {
        self.client.domain()
    }
    fn streaming_url(&self) -> &str {
        &self.client.streaming_url
    }
    async fn server_metadata(&self, _stored_kind: ServerKind) -> AdapterResult<ServerMetadata> {
        let meta = adapt_result(ServerKind::Misskey, self.client.get_meta().await)?;
        Ok(ServerMetadata {
            streaming_url: self.client.streaming_url.clone(),
            version: Some(meta.version.clone()),
            max_characters: normalize_limit(
                meta.max_note_text_length
                    .unwrap_or(MISSKEY_CHARACTER_LIMIT as i64),
                MISSKEY_CHARACTER_LIMIT,
            ),
            instance_json: serde_json::to_string(&meta).ok(),
            server_kind: ServerKind::Misskey,
        })
    }
}

#[async_trait]
impl SessionPort for BlueskyAdapter {
    fn kind(&self) -> ServerKind {
        ServerKind::Bluesky
    }
    fn access_token(&self) -> String {
        self.client.cached_access_token()
    }
    async fn current_access_token(&self) -> AdapterResult<String> {
        adapt_result(ServerKind::Bluesky, self.client.refresh_token().await)
    }
    fn domain(&self) -> &str {
        self.client.domain()
    }
    fn streaming_url(&self) -> &str {
        &self.client.streaming_url
    }
    fn set_bluesky_credential_sink(&self, sink: Arc<dyn BlueskyCredentialSink>) {
        self.client.set_credential_sink(sink);
    }
    async fn invalidate_auth_generation(&self) {
        self.client.invalidate_auth_generation().await;
    }
    fn bluesky_rate_limit_state(&self) -> Option<RateLimitState> {
        Some(self.client.rate_limit_state())
    }
    fn bluesky_app_password(&self) -> Option<String> {
        self.client.cached_app_password()
    }
    fn bluesky_polling_client(&self) -> Option<BlueskyClient> {
        Some(self.client.clone())
    }
    async fn server_metadata(&self, _stored_kind: ServerKind) -> AdapterResult<ServerMetadata> {
        Ok(ServerMetadata {
            streaming_url: self.client.streaming_url.clone(),
            version: None,
            max_characters: BLUESKY_CHARACTER_LIMIT,
            instance_json: serde_json::to_string(&serde_json::json!({
                "post": { "maxGraphemes": BLUESKY_CHARACTER_LIMIT, "maxBytes": 3000 }
            }))
            .ok(),
            server_kind: ServerKind::Bluesky,
        })
    }
}

macro_rules! direct_call {
    ($self:ident, $call:expr) => {
        adapt_result($self.kind(), $call.await)
    };
}

macro_rules! impl_direct_ports {
    ($adapter:ty) => {
        #[async_trait]
        impl TimelineReader for $adapter {
            async fn verify_credentials(&self) -> Result<Account, PortError> {
                direct_call!(self, self.client.verify_credentials())
            }
            async fn home(&self, params: &TimelineParams) -> Result<Vec<Status>, PortError> {
                direct_call!(self, self.client.get_home_timeline(params))
            }
            async fn public(
                &self,
                local: bool,
                params: &TimelineParams,
            ) -> Result<Vec<Status>, PortError> {
                direct_call!(self, self.client.get_public_timeline(local, params))
            }
            async fn list(
                &self,
                id: &str,
                params: &TimelineParams,
            ) -> Result<Vec<Status>, PortError> {
                direct_call!(self, self.client.get_list_timeline(id, params))
            }
            async fn feed(
                &self,
                _id: &str,
                _params: &TimelineParams,
            ) -> Result<Vec<Status>, PortError> {
                Err(adapt_error(
                    self.kind(),
                    LegacyProtocolError::IncompatibleInstance(
                        "Bluesky feeds are unavailable".to_string(),
                    ),
                ))
            }
            async fn hashtag(
                &self,
                tag: &str,
                local: bool,
                params: &TimelineParams,
            ) -> Result<Vec<Status>, PortError> {
                direct_call!(self, self.client.get_hashtag_timeline(tag, local, params))
            }
            async fn bookmarks(
                &self,
                params: &TimelineParams,
            ) -> Result<PaginatedResponse<Vec<Status>>, PortError> {
                direct_call!(self, self.client.get_bookmarks(params))
            }
            async fn favourites(
                &self,
                params: &TimelineParams,
            ) -> Result<PaginatedResponse<Vec<Status>>, PortError> {
                direct_call!(self, self.client.get_favourites(params))
            }
            async fn status(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.get_status(id))
            }
            async fn status_context(&self, id: &str) -> Result<StatusContext, PortError> {
                direct_call!(self, self.client.get_status_context(id))
            }
            async fn notifications(
                &self,
                params: &NotificationParams,
            ) -> Result<Vec<Notification>, PortError> {
                direct_call!(self, self.client.get_notifications(params))
            }
        }

        #[async_trait]
        impl StatusMutator for $adapter {
            async fn create(&self, params: &CreateStatusParams) -> Result<Status, PortError> {
                direct_call!(self, self.client.create_status(params))
            }
            async fn edit(
                &self,
                id: &str,
                params: &CreateStatusParams,
            ) -> Result<Status, PortError> {
                direct_call!(self, self.client.edit_status(id, params))
            }
            async fn delete(&self, id: &str) -> Result<(), PortError> {
                direct_call!(self, self.client.delete_status(id))
            }
            async fn favourite(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.favourite(id))
            }
            async fn unfavourite(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.unfavourite(id))
            }
            async fn reblog(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.reblog(id))
            }
            async fn unreblog(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.unreblog(id))
            }
            async fn bookmark(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.bookmark(id))
            }
            async fn unbookmark(&self, id: &str) -> Result<Status, PortError> {
                direct_call!(self, self.client.unbookmark(id))
            }
            async fn vote_poll(
                &self,
                id: &str,
                params: &VotePollParams,
            ) -> Result<Poll, PortError> {
                direct_call!(self, self.client.vote_poll(id, params))
            }
            async fn lookup_status_by_uri(&self, uri: &str) -> Result<Option<Status>, PortError> {
                direct_call!(self, self.client.lookup_status_by_uri(uri))
            }
        }

        #[async_trait]
        impl RelationshipManager for $adapter {
            async fn account(&self, id: &str) -> Result<Account, PortError> {
                direct_call!(self, self.client.get_account(id))
            }
            async fn account_statuses(
                &self,
                id: &str,
                params: &AccountStatusesParams,
            ) -> Result<PaginatedResponse<Vec<Status>>, PortError> {
                direct_call!(self, self.client.get_account_statuses(id, params))
            }
            async fn relationships(&self, ids: &[&str]) -> Result<Vec<Relationship>, PortError> {
                direct_call!(self, self.client.get_relationships(ids))
            }
            async fn follow(&self, id: &str) -> Result<Relationship, PortError> {
                direct_call!(self, self.client.follow_account(id))
            }
            async fn unfollow(&self, id: &str) -> Result<Relationship, PortError> {
                direct_call!(self, self.client.unfollow_account(id))
            }
            async fn mute(&self, id: &str) -> Result<Relationship, PortError> {
                direct_call!(self, self.client.mute_account(id))
            }
            async fn unmute(&self, id: &str) -> Result<Relationship, PortError> {
                direct_call!(self, self.client.unmute_account(id))
            }
            async fn block(&self, id: &str) -> Result<Relationship, PortError> {
                direct_call!(self, self.client.block_account(id))
            }
            async fn unblock(&self, id: &str) -> Result<Relationship, PortError> {
                direct_call!(self, self.client.unblock_account(id))
            }
        }

        #[async_trait]
        impl DiscoveryReader for $adapter {
            async fn lists(&self) -> Result<Vec<List>, PortError> {
                direct_call!(self, self.client.get_lists())
            }
            async fn feeds(&self) -> Result<Vec<List>, PortError> {
                Err(adapt_error(
                    self.kind(),
                    LegacyProtocolError::IncompatibleInstance(
                        "Bluesky feeds are unavailable".to_string(),
                    ),
                ))
            }
            async fn custom_emojis(&self) -> Result<Vec<CustomEmoji>, PortError> {
                direct_call!(self, self.client.get_custom_emojis())
            }
            async fn search_accounts(
                &self,
                query: &str,
                limit: u32,
            ) -> Result<Vec<Account>, PortError> {
                direct_call!(self, self.client.search_accounts(query, limit))
            }
            async fn search_hashtags(
                &self,
                query: &str,
                limit: u32,
            ) -> Result<SearchResult, PortError> {
                direct_call!(self, self.client.search_hashtags(query, limit))
            }
        }

        #[async_trait]
        impl MediaUploader for $adapter {
            async fn upload(&self, path: &Path) -> Result<MediaAttachment, PortError> {
                direct_call!(self, self.client.upload_media(path))
            }
        }
    };
}

impl_direct_ports!(MastodonAdapter);
impl_direct_ports!(MisskeyAdapter);

macro_rules! bluesky_call {
    ($self:ident, $label:literal, $method:ident ( $($argument:expr),* $(,)? )) => {
        $self
            .with_auth_retry($label, || $self.client.$method($($argument),*))
            .await
            .map_err(|error| adapt_error(ServerKind::Bluesky, error))
    };
}

#[async_trait]
impl TimelineReader for BlueskyAdapter {
    async fn verify_credentials(&self) -> Result<Account, PortError> {
        bluesky_call!(self, "verify_credentials", verify_credentials())
    }
    async fn home(&self, params: &TimelineParams) -> Result<Vec<Status>, PortError> {
        bluesky_call!(self, "get_home_timeline", get_home_timeline(params))
    }
    async fn public(&self, local: bool, params: &TimelineParams) -> Result<Vec<Status>, PortError> {
        bluesky_call!(
            self,
            "get_public_timeline",
            get_public_timeline(local, params)
        )
    }
    async fn list(&self, id: &str, params: &TimelineParams) -> Result<Vec<Status>, PortError> {
        bluesky_call!(self, "get_list_timeline", get_list_timeline(id, params))
    }
    async fn feed(&self, id: &str, params: &TimelineParams) -> Result<Vec<Status>, PortError> {
        bluesky_call!(self, "get_feed_timeline", get_feed_timeline(id, params))
    }
    async fn hashtag(
        &self,
        tag: &str,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, PortError> {
        bluesky_call!(
            self,
            "get_hashtag_timeline",
            get_hashtag_timeline(tag, local, params)
        )
    }
    async fn bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, PortError> {
        bluesky_call!(self, "get_bookmarks", get_bookmarks(params))
    }
    async fn favourites(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, PortError> {
        bluesky_call!(self, "get_favourites", get_favourites(params))
    }
    async fn status(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "get_status", get_status(id))
    }
    async fn status_context(&self, id: &str) -> Result<StatusContext, PortError> {
        bluesky_call!(self, "get_status_context", get_status_context(id))
    }
    async fn notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, PortError> {
        bluesky_call!(self, "get_notifications", get_notifications(params))
    }
}

#[async_trait]
impl StatusMutator for BlueskyAdapter {
    async fn create(&self, params: &CreateStatusParams) -> Result<Status, PortError> {
        bluesky_call!(self, "create_status", create_status(params))
    }
    async fn edit(&self, id: &str, params: &CreateStatusParams) -> Result<Status, PortError> {
        bluesky_call!(self, "edit_status", edit_status(id, params))
    }
    async fn delete(&self, id: &str) -> Result<(), PortError> {
        bluesky_call!(self, "delete_status", delete_status(id))
    }
    async fn favourite(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "favourite", favourite(id))
    }
    async fn unfavourite(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "unfavourite", unfavourite(id))
    }
    async fn reblog(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "reblog", reblog(id))
    }
    async fn unreblog(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "unreblog", unreblog(id))
    }
    async fn bookmark(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "bookmark", bookmark(id))
    }
    async fn unbookmark(&self, id: &str) -> Result<Status, PortError> {
        bluesky_call!(self, "unbookmark", unbookmark(id))
    }
    async fn vote_poll(&self, id: &str, params: &VotePollParams) -> Result<Poll, PortError> {
        bluesky_call!(self, "vote_poll", vote_poll(id, params))
    }
    async fn lookup_status_by_uri(&self, uri: &str) -> Result<Option<Status>, PortError> {
        bluesky_call!(self, "lookup_status_by_uri", lookup_status_by_uri(uri))
    }
}

#[async_trait]
impl RelationshipManager for BlueskyAdapter {
    async fn account(&self, id: &str) -> Result<Account, PortError> {
        bluesky_call!(self, "get_account", get_account(id))
    }
    async fn account_statuses(
        &self,
        id: &str,
        params: &AccountStatusesParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, PortError> {
        bluesky_call!(
            self,
            "get_account_statuses",
            get_account_statuses(id, params)
        )
    }
    async fn relationships(&self, ids: &[&str]) -> Result<Vec<Relationship>, PortError> {
        bluesky_call!(self, "get_relationships", get_relationships(ids))
    }
    async fn follow(&self, id: &str) -> Result<Relationship, PortError> {
        bluesky_call!(self, "follow_account", follow_account(id))
    }
    async fn unfollow(&self, id: &str) -> Result<Relationship, PortError> {
        bluesky_call!(self, "unfollow_account", unfollow_account(id))
    }
    async fn mute(&self, id: &str) -> Result<Relationship, PortError> {
        bluesky_call!(self, "mute_account", mute_account(id))
    }
    async fn unmute(&self, id: &str) -> Result<Relationship, PortError> {
        bluesky_call!(self, "unmute_account", unmute_account(id))
    }
    async fn block(&self, id: &str) -> Result<Relationship, PortError> {
        bluesky_call!(self, "block_account", block_account(id))
    }
    async fn unblock(&self, id: &str) -> Result<Relationship, PortError> {
        bluesky_call!(self, "unblock_account", unblock_account(id))
    }
}

#[async_trait]
impl DiscoveryReader for BlueskyAdapter {
    async fn lists(&self) -> Result<Vec<List>, PortError> {
        bluesky_call!(self, "get_lists", get_lists())
    }
    async fn feeds(&self) -> Result<Vec<List>, PortError> {
        bluesky_call!(self, "get_saved_feeds", get_saved_feeds())
    }
    async fn custom_emojis(&self) -> Result<Vec<CustomEmoji>, PortError> {
        bluesky_call!(self, "get_custom_emojis", get_custom_emojis())
    }
    async fn search_accounts(&self, query: &str, limit: u32) -> Result<Vec<Account>, PortError> {
        bluesky_call!(self, "search_accounts", search_accounts(query, limit))
    }
    async fn search_hashtags(&self, query: &str, limit: u32) -> Result<SearchResult, PortError> {
        bluesky_call!(self, "search_hashtags", search_hashtags(query, limit))
    }
}

#[async_trait]
impl MediaUploader for BlueskyAdapter {
    async fn upload(&self, path: &Path) -> Result<MediaAttachment, PortError> {
        bluesky_call!(self, "upload_media", upload_media(path))
    }
}

fn normalize_limit(value: i64, fallback: i32) -> i32 {
    if value <= 0 {
        fallback
    } else {
        i32::try_from(value).unwrap_or(fallback)
    }
}
