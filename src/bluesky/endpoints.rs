//! Mastodon-shaped wrappers around Bluesky (AT Protocol) endpoints.
//!
//! Each method here mirrors the corresponding `MastodonClient` method so the rest
//! of the app can stay backend-agnostic. Internally we hit the appropriate
//! `app.bsky.*` / `com.atproto.*` endpoint and run the result through
//! `crate::bluesky::convert`.
//!
//! Many Mastodon concepts (custom emojis, polls, multi-host federation, edits)
//! don't have direct Bluesky equivalents — those methods return empty results
//! or sensible no-ops.

use std::path::Path;

use atrium_api::app::bsky::actor::get_profile::ParametersData as GetProfileParams;
use atrium_api::app::bsky::feed::get_author_feed::ParametersData as GetAuthorFeedParams;
use atrium_api::app::bsky::feed::get_post_thread::ParametersData as GetPostThreadParams;
use atrium_api::app::bsky::feed::get_posts::ParametersData as GetPostsParams;
use atrium_api::app::bsky::feed::get_timeline::ParametersData as GetTimelineParams;
use atrium_api::app::bsky::notification::list_notifications::ParametersData as ListNotificationsParams;
use atrium_api::com::atproto::repo::create_record::InputData as CreateRecordInput;
use atrium_api::com::atproto::repo::delete_record::InputData as DeleteRecordInput;
use atrium_api::com::atproto::repo::strong_ref::MainData as StrongRefData;
use atrium_api::types::string::{AtIdentifier, Datetime, Did, Handle, Nsid, RecordKey};
use atrium_api::types::{Collection, LimitedNonZeroU8, LimitedU16, TryFromUnknown, Union};

use crate::bluesky::client::BlueskyClient;
use crate::bluesky::convert::{
    feed_view_post_to_status, post_view_to_status, profile_basic_to_account,
    profile_detailed_to_account, text_to_html, BSKY_APP_HOST,
};
use crate::mastodon::client::PaginatedResponse;
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::statuses::{CreateStatusParams, VotePollParams};
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::{Account, CustomEmoji, Relationship};
use crate::mastodon::types::list::List;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::search::SearchResult;
use crate::mastodon::types::status::{MediaAttachment, Poll, Status, StatusContext, StatusSource};

fn err(msg: impl Into<String>) -> MastodonError {
    MastodonError::Other(msg.into())
}

fn timeline_limit(params: &TimelineParams) -> Option<LimitedNonZeroU8<100>> {
    let limit = params.limit.unwrap_or(40).clamp(1, 100) as u8;
    LimitedNonZeroU8::<100>::try_from(limit).ok()
}

impl BlueskyClient {
    pub async fn verify_credentials(&self) -> Result<Account, MastodonError> {
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let did = session.data.did.clone();
        let actor = AtIdentifier::Did(did);
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .actor
            .get_profile(GetProfileParams { actor }.into())
            .await
            .map_err(|e| err(format!("get_profile failed: {}", e)))?;
        Ok(profile_detailed_to_account(&resp))
    }

    pub async fn get_home_timeline(
        &self,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_timeline(
                GetTimelineParams {
                    algorithm: None,
                    cursor: params.max_id.clone(),
                    limit: timeline_limit(params),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_timeline failed: {}", e)))?;

        Ok(resp.feed.iter().map(feed_view_post_to_status).collect())
    }

    pub async fn get_public_timeline(
        &self,
        _local: bool,
        _params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        // Bluesky does not expose Mastodon-style public/local feeds. Return an
        // empty list so the panel renders gracefully.
        Ok(Vec::new())
    }

    pub async fn get_list_timeline(
        &self,
        list_id: &str,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        use atrium_api::app::bsky::feed::get_list_feed::ParametersData as GetListFeedParams;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_list_feed(
                GetListFeedParams {
                    list: list_id.to_string(),
                    cursor: params.max_id.clone(),
                    limit: timeline_limit(params),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_list_feed failed: {}", e)))?;
        Ok(resp.feed.iter().map(feed_view_post_to_status).collect())
    }

    pub async fn get_hashtag_timeline(
        &self,
        tag: &str,
        _local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        use atrium_api::app::bsky::feed::search_posts::ParametersData as SearchPostsParams;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .search_posts(
                SearchPostsParams {
                    q: format!("#{}", tag),
                    cursor: params.max_id.clone(),
                    limit: timeline_limit(params),
                    sort: Some("latest".to_string()),
                    author: None,
                    domain: None,
                    lang: None,
                    mentions: None,
                    since: None,
                    tag: Some(vec![tag.to_string()]),
                    until: None,
                    url: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("search_posts failed: {}", e)))?;
        Ok(resp.posts.iter().map(post_view_to_status).collect())
    }

    pub async fn get_bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, MastodonError> {
        use atrium_api::app::bsky::bookmark::defs::BookmarkViewItemRefs;
        use atrium_api::app::bsky::bookmark::get_bookmarks::ParametersData as GetBookmarksParams;

        let limit = params.limit.unwrap_or(40).clamp(1, 100) as u8;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .bookmark
            .get_bookmarks(
                GetBookmarksParams {
                    cursor: params.max_id.clone(),
                    limit: LimitedNonZeroU8::<100>::try_from(limit).ok(),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_bookmarks failed: {}", e)))?;

        let mut data: Vec<Status> = Vec::new();
        for view in &resp.bookmarks {
            // Skip blocked / not-found posts — surface only successfully-resolved ones.
            if let Union::Refs(BookmarkViewItemRefs::AppBskyFeedDefsPostView(post)) =
                &view.data.item
            {
                let mut status = post_view_to_status(post);
                // The bookmark namespace doesn't populate the post viewer's
                // `bookmarked` flag for us — but everything in this response
                // IS bookmarked, so set it explicitly so the DB writer marks it.
                status.bookmarked = Some(true);
                data.push(status);
            }
        }
        Ok(PaginatedResponse {
            data,
            next_max_id: resp.cursor.clone(),
        })
    }

    pub async fn get_status(&self, id: &str) -> Result<Status, MastodonError> {
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_posts(
                GetPostsParams {
                    uris: vec![id.to_string()],
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_posts failed: {}", e)))?;
        let post = resp
            .posts
            .first()
            .ok_or_else(|| err("Bluesky post not found"))?;
        Ok(post_view_to_status(post))
    }

    pub async fn lookup_status_by_uri(
        &self,
        uri: &str,
    ) -> Result<Option<Status>, MastodonError> {
        if !uri.starts_with("at://") {
            return Ok(None);
        }
        match self.get_status(uri).await {
            Ok(s) => Ok(Some(s)),
            Err(_) => Ok(None),
        }
    }

    pub async fn get_status_context(&self, id: &str) -> Result<StatusContext, MastodonError> {
        use atrium_api::app::bsky::feed::defs::ThreadViewPostParentRefs;
        use atrium_api::app::bsky::feed::get_post_thread::OutputThreadRefs;

        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_post_thread(
                GetPostThreadParams {
                    uri: id.to_string(),
                    depth: LimitedU16::<1000>::try_from(12u16).ok(),
                    parent_height: LimitedU16::<1000>::try_from(80u16).ok(),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_post_thread failed: {}", e)))?;

        let mut ancestors: Vec<Status> = Vec::new();
        let mut descendants: Vec<Status> = Vec::new();

        if let Union::Refs(OutputThreadRefs::AppBskyFeedDefsThreadViewPost(thread)) = &resp.thread {
            // Walk parent chain upward.
            let mut current_parent = thread.data.parent.clone();
            while let Some(Union::Refs(ThreadViewPostParentRefs::ThreadViewPost(parent))) =
                current_parent
            {
                ancestors.push(post_view_to_status(&parent.data.post));
                current_parent = parent.data.parent.clone();
            }
            ancestors.reverse();

            // Walk replies depth-first.
            if let Some(replies) = &thread.data.replies {
                for reply in replies {
                    collect_descendants(reply, &mut descendants);
                }
            }
        }

        Ok(StatusContext {
            ancestors,
            descendants,
        })
    }

    pub async fn get_status_source(&self, id: &str) -> Result<StatusSource, MastodonError> {
        let status = self.get_status(id).await?;
        Ok(StatusSource {
            id: status.id,
            text: html_to_plain(&status.content),
            spoiler_text: status.spoiler_text,
        })
    }

    pub async fn create_status(
        &self,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let repo = AtIdentifier::Did(session.data.did.clone());

        let text = params.status.clone().unwrap_or_default();
        let mut record_data = atrium_api::app::bsky::feed::post::RecordData {
            created_at: Datetime::now(),
            embed: None,
            entities: None,
            facets: None,
            labels: None,
            langs: None,
            reply: None,
            tags: None,
            text,
        };

        if let Some(reply_to) = params.in_reply_to_id.as_ref() {
            if let Ok(reply_ref) = self.build_reply_ref(reply_to).await {
                record_data.reply = Some(reply_ref);
            }
        }

        let nsid: Nsid = atrium_api::app::bsky::feed::Post::nsid();
        let record = serialize_to_unknown(&record_data)?;
        let resp = self
            .agent()
            .api
            .com
            .atproto
            .repo
            .create_record(
                CreateRecordInput {
                    collection: nsid,
                    record,
                    repo,
                    rkey: None,
                    swap_commit: None,
                    validate: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("create_record(post) failed: {}", e)))?;

        // Fetch the freshly-created post for return.
        self.get_status(&resp.uri).await
    }

    pub async fn edit_status(
        &self,
        _id: &str,
        _params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        Err(err("Bluesky does not support editing posts"))
    }

    pub async fn delete_status(&self, id: &str) -> Result<(), MastodonError> {
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let (collection, rkey) = at_uri_split(id)?;
        self.agent()
            .api
            .com
            .atproto
            .repo
            .delete_record(
                DeleteRecordInput {
                    collection,
                    repo: AtIdentifier::Did(session.data.did.clone()),
                    rkey,
                    swap_commit: None,
                    swap_record: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("delete_record failed: {}", e)))?;
        Ok(())
    }

    pub async fn favourite(&self, id: &str) -> Result<Status, MastodonError> {
        let strong = self.fetch_strong_ref(id).await?;
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let record_data = atrium_api::app::bsky::feed::like::RecordData {
            created_at: Datetime::now(),
            subject: strong.into(),
            via: None,
        };
        let nsid: Nsid = atrium_api::app::bsky::feed::Like::nsid();
        let record = serialize_to_unknown(&record_data)?;
        self.agent()
            .api
            .com
            .atproto
            .repo
            .create_record(
                CreateRecordInput {
                    collection: nsid,
                    record,
                    repo: AtIdentifier::Did(session.data.did.clone()),
                    rkey: None,
                    swap_commit: None,
                    validate: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("create_record(like) failed: {}", e)))?;
        self.get_status(id).await
    }

    pub async fn unfavourite(&self, id: &str) -> Result<Status, MastodonError> {
        if let Some(uri) = self.viewer_like_uri(id).await? {
            let session = self
                .agent()
                .get_session()
                .await
                .ok_or_else(|| err("Bluesky agent has no session"))?;
            let (collection, rkey) = at_uri_split(&uri)?;
            self.agent()
                .api
                .com
                .atproto
                .repo
                .delete_record(
                    DeleteRecordInput {
                        collection,
                        repo: AtIdentifier::Did(session.data.did.clone()),
                        rkey,
                        swap_commit: None,
                        swap_record: None,
                    }
                    .into(),
                )
                .await
                .map_err(|e| err(format!("delete_record(like) failed: {}", e)))?;
        }
        self.get_status(id).await
    }

    pub async fn reblog(&self, id: &str) -> Result<Status, MastodonError> {
        let strong = self.fetch_strong_ref(id).await?;
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let record_data = atrium_api::app::bsky::feed::repost::RecordData {
            created_at: Datetime::now(),
            subject: strong.into(),
            via: None,
        };
        let nsid: Nsid = atrium_api::app::bsky::feed::Repost::nsid();
        let record = serialize_to_unknown(&record_data)?;
        self.agent()
            .api
            .com
            .atproto
            .repo
            .create_record(
                CreateRecordInput {
                    collection: nsid,
                    record,
                    repo: AtIdentifier::Did(session.data.did.clone()),
                    rkey: None,
                    swap_commit: None,
                    validate: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("create_record(repost) failed: {}", e)))?;
        self.get_status(id).await
    }

    pub async fn unreblog(&self, id: &str) -> Result<Status, MastodonError> {
        if let Some(uri) = self.viewer_repost_uri(id).await? {
            let session = self
                .agent()
                .get_session()
                .await
                .ok_or_else(|| err("Bluesky agent has no session"))?;
            let (collection, rkey) = at_uri_split(&uri)?;
            self.agent()
                .api
                .com
                .atproto
                .repo
                .delete_record(
                    DeleteRecordInput {
                        collection,
                        repo: AtIdentifier::Did(session.data.did.clone()),
                        rkey,
                        swap_commit: None,
                        swap_record: None,
                    }
                    .into(),
                )
                .await
                .map_err(|e| err(format!("delete_record(repost) failed: {}", e)))?;
        }
        self.get_status(id).await
    }

    pub async fn bookmark(&self, id: &str) -> Result<Status, MastodonError> {
        use atrium_api::app::bsky::bookmark::create_bookmark::InputData as CreateBookmarkInput;

        let strong = self.fetch_strong_ref(id).await?;
        self.agent()
            .api
            .app
            .bsky
            .bookmark
            .create_bookmark(
                CreateBookmarkInput {
                    cid: strong.cid.clone(),
                    uri: strong.uri.clone(),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("create_bookmark failed: {}", e)))?;
        let mut status = self.get_status(id).await?;
        status.bookmarked = Some(true);
        Ok(status)
    }

    pub async fn unbookmark(&self, id: &str) -> Result<Status, MastodonError> {
        use atrium_api::app::bsky::bookmark::delete_bookmark::InputData as DeleteBookmarkInput;

        self.agent()
            .api
            .app
            .bsky
            .bookmark
            .delete_bookmark(
                DeleteBookmarkInput {
                    uri: id.to_string(),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("delete_bookmark failed: {}", e)))?;
        let mut status = self.get_status(id).await?;
        status.bookmarked = Some(false);
        Ok(status)
    }

    pub async fn get_poll(&self, _id: &str) -> Result<Poll, MastodonError> {
        Err(err("Bluesky does not have polls"))
    }

    pub async fn vote_poll(
        &self,
        _id: &str,
        _params: &VotePollParams,
    ) -> Result<Poll, MastodonError> {
        Err(err("Bluesky does not have polls"))
    }

    pub async fn get_notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, MastodonError> {
        let limit = params.limit.unwrap_or(30).clamp(1, 100) as u8;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .notification
            .list_notifications(
                ListNotificationsParams {
                    cursor: params.max_id.clone(),
                    limit: LimitedNonZeroU8::<100>::try_from(limit).ok(),
                    priority: None,
                    reasons: None,
                    seen_at: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("list_notifications failed: {}", e)))?;

        // Pre-fetch hydrated posts for any notification that references one
        // (like / repost / quote / reply / mention all carry a reason_subject AT-URI).
        let mut subject_uris: Vec<String> = Vec::new();
        for n in &resp.notifications {
            if let Some(uri) = n.data.reason_subject.as_ref() {
                if !subject_uris.contains(uri) {
                    subject_uris.push(uri.clone());
                }
            }
        }
        let mut subject_lookup: std::collections::HashMap<String, Status> =
            std::collections::HashMap::new();
        if !subject_uris.is_empty() {
            // Bluesky's getPosts caps at 25 URIs per call.
            for chunk in subject_uris.chunks(25) {
                if let Ok(posts) = self
                    .agent()
                    .api
                    .app
                    .bsky
                    .feed
                    .get_posts(
                        GetPostsParams {
                            uris: chunk.to_vec(),
                        }
                        .into(),
                    )
                    .await
                {
                    for post in &posts.posts {
                        let status = post_view_to_status(post);
                        subject_lookup.insert(post.data.uri.clone(), status);
                    }
                }
            }
        }

        let mut out = Vec::new();
        for n in &resp.notifications {
            if let Some(converted) = convert_notification(n, &subject_lookup) {
                out.push(converted);
            }
        }
        Ok(out)
    }

    pub async fn get_notification(&self, _id: &str) -> Result<Notification, MastodonError> {
        Err(err(
            "Bluesky does not expose a single-notification fetch endpoint",
        ))
    }

    pub async fn dismiss_notification(&self, _id: &str) -> Result<(), MastodonError> {
        // Bluesky has updateSeen instead of per-notification dismiss; treat as noop.
        Ok(())
    }

    pub async fn get_account(&self, id: &str) -> Result<Account, MastodonError> {
        let actor = parse_actor(id)?;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .actor
            .get_profile(GetProfileParams { actor }.into())
            .await
            .map_err(|e| err(format!("get_profile failed: {}", e)))?;
        Ok(profile_detailed_to_account(&resp))
    }

    pub async fn get_account_statuses(
        &self,
        id: &str,
        params: &AccountStatusesParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let actor = parse_actor(id)?;
        let limit = params.limit.unwrap_or(20).clamp(1, 100) as u8;
        let filter = if params.only_media.unwrap_or(false) {
            Some("posts_with_media".to_string())
        } else if params.exclude_replies.unwrap_or(false) {
            Some("posts_no_replies".to_string())
        } else if params.exclude_reblogs.unwrap_or(false) {
            Some("posts_and_author_threads".to_string())
        } else {
            None
        };
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_author_feed(
                GetAuthorFeedParams {
                    actor,
                    cursor: params.max_id.clone(),
                    filter,
                    include_pins: None,
                    limit: LimitedNonZeroU8::<100>::try_from(limit).ok(),
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_author_feed failed: {}", e)))?;
        Ok(resp.feed.iter().map(feed_view_post_to_status).collect())
    }

    pub async fn get_relationships(
        &self,
        ids: &[&str],
    ) -> Result<Vec<Relationship>, MastodonError> {
        // Without batched relationships in Mastodon shape, return defaults.
        Ok(ids
            .iter()
            .map(|id| Relationship {
                id: (*id).to_string(),
                following: false,
                followed_by: false,
                blocking: false,
                blocked_by: false,
                muting: false,
                requested: false,
                note: String::new(),
            })
            .collect())
    }

    pub async fn follow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let target_did = self.resolve_did(id).await?;
        let record_data = atrium_api::app::bsky::graph::follow::RecordData {
            created_at: Datetime::now(),
            subject: target_did,
        };
        let nsid: Nsid = atrium_api::app::bsky::graph::Follow::nsid();
        let record = serialize_to_unknown(&record_data)?;
        self.agent()
            .api
            .com
            .atproto
            .repo
            .create_record(
                CreateRecordInput {
                    collection: nsid,
                    record,
                    repo: AtIdentifier::Did(session.data.did.clone()),
                    rkey: None,
                    swap_commit: None,
                    validate: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("create_record(follow) failed: {}", e)))?;
        Ok(Relationship {
            id: id.to_string(),
            following: true,
            followed_by: false,
            blocking: false,
            blocked_by: false,
            muting: false,
            requested: false,
            note: String::new(),
        })
    }

    pub async fn unfollow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        // We need the URI of the follow record to delete. Look it up via getProfile.viewer.following.
        let actor = parse_actor(id)?;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .actor
            .get_profile(GetProfileParams { actor }.into())
            .await
            .map_err(|e| err(format!("get_profile failed: {}", e)))?;

        if let Some(viewer) = resp.viewer.as_ref() {
            if let Some(follow_uri) = viewer.following.as_ref() {
                let session = self
                    .agent()
                    .get_session()
                    .await
                    .ok_or_else(|| err("Bluesky agent has no session"))?;
                let (collection, rkey) = at_uri_split(follow_uri)?;
                self.agent()
                    .api
                    .com
                    .atproto
                    .repo
                    .delete_record(
                        DeleteRecordInput {
                            collection,
                            repo: AtIdentifier::Did(session.data.did.clone()),
                            rkey,
                            swap_commit: None,
                            swap_record: None,
                        }
                        .into(),
                    )
                    .await
                    .map_err(|e| err(format!("delete_record(follow) failed: {}", e)))?;
            }
        }
        Ok(Relationship {
            id: id.to_string(),
            following: false,
            followed_by: false,
            blocking: false,
            blocked_by: false,
            muting: false,
            requested: false,
            note: String::new(),
        })
    }

    pub async fn mute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        use atrium_api::app::bsky::graph::mute_actor::InputData as MuteActorInput;
        let actor = parse_actor(id)?;
        self.agent()
            .api
            .app
            .bsky
            .graph
            .mute_actor(MuteActorInput { actor }.into())
            .await
            .map_err(|e| err(format!("mute_actor failed: {}", e)))?;
        Ok(Relationship {
            id: id.to_string(),
            following: false,
            followed_by: false,
            blocking: false,
            blocked_by: false,
            muting: true,
            requested: false,
            note: String::new(),
        })
    }

    pub async fn unmute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        use atrium_api::app::bsky::graph::unmute_actor::InputData as UnmuteActorInput;
        let actor = parse_actor(id)?;
        self.agent()
            .api
            .app
            .bsky
            .graph
            .unmute_actor(UnmuteActorInput { actor }.into())
            .await
            .map_err(|e| err(format!("unmute_actor failed: {}", e)))?;
        Ok(Relationship {
            id: id.to_string(),
            following: false,
            followed_by: false,
            blocking: false,
            blocked_by: false,
            muting: false,
            requested: false,
            note: String::new(),
        })
    }

    pub async fn block_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let session = self
            .agent()
            .get_session()
            .await
            .ok_or_else(|| err("Bluesky agent has no session"))?;
        let target_did = self.resolve_did(id).await?;
        let record_data = atrium_api::app::bsky::graph::block::RecordData {
            created_at: Datetime::now(),
            subject: target_did,
        };
        let nsid: Nsid = atrium_api::app::bsky::graph::Block::nsid();
        let record = serialize_to_unknown(&record_data)?;
        self.agent()
            .api
            .com
            .atproto
            .repo
            .create_record(
                CreateRecordInput {
                    collection: nsid,
                    record,
                    repo: AtIdentifier::Did(session.data.did.clone()),
                    rkey: None,
                    swap_commit: None,
                    validate: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("create_record(block) failed: {}", e)))?;
        Ok(Relationship {
            id: id.to_string(),
            following: false,
            followed_by: false,
            blocking: true,
            blocked_by: false,
            muting: false,
            requested: false,
            note: String::new(),
        })
    }

    pub async fn unblock_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let actor = parse_actor(id)?;
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .actor
            .get_profile(GetProfileParams { actor }.into())
            .await
            .map_err(|e| err(format!("get_profile failed: {}", e)))?;
        if let Some(viewer) = resp.viewer.as_ref() {
            if let Some(block_uri) = viewer.blocking.as_ref() {
                let session = self
                    .agent()
                    .get_session()
                    .await
                    .ok_or_else(|| err("Bluesky agent has no session"))?;
                let (collection, rkey) = at_uri_split(block_uri)?;
                self.agent()
                    .api
                    .com
                    .atproto
                    .repo
                    .delete_record(
                        DeleteRecordInput {
                            collection,
                            repo: AtIdentifier::Did(session.data.did.clone()),
                            rkey,
                            swap_commit: None,
                            swap_record: None,
                        }
                        .into(),
                    )
                    .await
                    .map_err(|e| err(format!("delete_record(block) failed: {}", e)))?;
            }
        }
        Ok(Relationship {
            id: id.to_string(),
            following: false,
            followed_by: false,
            blocking: false,
            blocked_by: false,
            muting: false,
            requested: false,
            note: String::new(),
        })
    }

    pub async fn get_lists(&self) -> Result<Vec<List>, MastodonError> {
        // Lists API exists but requires user iteration; defer.
        Ok(Vec::new())
    }

    pub async fn get_custom_emojis(&self) -> Result<Vec<CustomEmoji>, MastodonError> {
        // Bluesky does not have custom emojis.
        Ok(Vec::new())
    }

    pub async fn search_accounts(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Account>, MastodonError> {
        use atrium_api::app::bsky::actor::search_actors::ParametersData as SearchActorsParams;
        let limit = (limit as u8).clamp(1, 100);
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .actor
            .search_actors(
                SearchActorsParams {
                    cursor: None,
                    limit: LimitedNonZeroU8::<100>::try_from(limit).ok(),
                    q: Some(query.to_string()),
                    term: None,
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("search_actors failed: {}", e)))?;
        Ok(resp
            .actors
            .iter()
            .map(|a| profile_basic_to_account(&actor_view_to_basic(a)))
            .collect())
    }

    pub async fn search_hashtags(
        &self,
        _query: &str,
        _limit: u32,
    ) -> Result<SearchResult, MastodonError> {
        Ok(SearchResult {
            accounts: Vec::new(),
            statuses: Vec::new(),
            hashtags: Vec::new(),
        })
    }

    pub async fn upload_media(
        &self,
        _file_path: &Path,
    ) -> Result<MediaAttachment, MastodonError> {
        Err(err("Bluesky media upload is not implemented yet"))
    }

    // -- internal helpers -----------------------------------------------------

    /// Resolve an actor identifier (handle or DID) to a DID. Bluesky `follow` /
    /// `block` records require the subject as a DID.
    async fn resolve_did(&self, id: &str) -> Result<Did, MastodonError> {
        if let Ok(did) = id.parse::<Did>() {
            return Ok(did);
        }
        // Treat as handle and look up via getProfile, which returns the canonical DID.
        let actor = parse_actor(id)?;
        let profile = self
            .agent()
            .api
            .app
            .bsky
            .actor
            .get_profile(GetProfileParams { actor }.into())
            .await
            .map_err(|e| err(format!("get_profile failed during DID resolve: {}", e)))?;
        Ok(profile.data.did.clone())
    }

    async fn fetch_strong_ref(&self, uri: &str) -> Result<StrongRefData, MastodonError> {
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_posts(
                GetPostsParams {
                    uris: vec![uri.to_string()],
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_posts failed: {}", e)))?;
        let post = resp
            .posts
            .first()
            .ok_or_else(|| err("Bluesky post not found"))?;
        Ok(StrongRefData {
            cid: post.data.cid.clone(),
            uri: post.data.uri.clone(),
        })
    }

    async fn viewer_like_uri(&self, uri: &str) -> Result<Option<String>, MastodonError> {
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_posts(
                GetPostsParams {
                    uris: vec![uri.to_string()],
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_posts failed: {}", e)))?;
        Ok(resp
            .posts
            .first()
            .and_then(|p| p.data.viewer.as_ref())
            .and_then(|v| v.data.like.clone()))
    }

    async fn viewer_repost_uri(&self, uri: &str) -> Result<Option<String>, MastodonError> {
        let resp = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_posts(
                GetPostsParams {
                    uris: vec![uri.to_string()],
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_posts failed: {}", e)))?;
        Ok(resp
            .posts
            .first()
            .and_then(|p| p.data.viewer.as_ref())
            .and_then(|v| v.data.repost.clone()))
    }

    async fn build_reply_ref(
        &self,
        parent_uri: &str,
    ) -> Result<atrium_api::app::bsky::feed::post::ReplyRef, MastodonError> {
        let parent_strong = self.fetch_strong_ref(parent_uri).await?;

        // Walk up to the thread root if parent itself is a reply.
        let parent_post = self
            .agent()
            .api
            .app
            .bsky
            .feed
            .get_posts(
                GetPostsParams {
                    uris: vec![parent_uri.to_string()],
                }
                .into(),
            )
            .await
            .map_err(|e| err(format!("get_posts failed: {}", e)))?;
        let parent_post_view = parent_post
            .posts
            .first()
            .ok_or_else(|| err("Bluesky reply parent not found"))?;

        let root = match atrium_api::app::bsky::feed::post::Record::try_from_unknown(
            parent_post_view.data.record.clone(),
        ) {
            Ok(rec) => match rec.data.reply {
                Some(parent_reply) => parent_reply.data.root.data.clone(),
                None => parent_strong.clone(),
            },
            Err(_) => parent_strong.clone(),
        };

        let reply_data = atrium_api::app::bsky::feed::post::ReplyRefData {
            parent: parent_strong.into(),
            root: root.into(),
        };
        Ok(reply_data.into())
    }
}

fn at_uri_split(uri: &str) -> Result<(Nsid, RecordKey), MastodonError> {
    // at://did:.../<collection>/<rkey>
    let rest = uri
        .strip_prefix("at://")
        .ok_or_else(|| err(format!("Not an AT-URI: {}", uri)))?;
    let mut parts = rest.splitn(3, '/');
    parts.next().ok_or_else(|| err("AT-URI missing repo"))?;
    let coll = parts
        .next()
        .ok_or_else(|| err("AT-URI missing collection"))?;
    let rkey = parts.next().ok_or_else(|| err("AT-URI missing rkey"))?;
    let nsid: Nsid = coll
        .parse()
        .map_err(|e| err(format!("Invalid NSID '{}': {:?}", coll, e)))?;
    let rkey: RecordKey = rkey
        .parse()
        .map_err(|e| err(format!("Invalid rkey '{}': {:?}", rkey, e)))?;
    Ok((nsid, rkey))
}

fn parse_actor(id: &str) -> Result<AtIdentifier, MastodonError> {
    if let Ok(did) = id.parse::<Did>() {
        return Ok(AtIdentifier::Did(did));
    }
    if let Ok(handle) = id.parse::<Handle>() {
        return Ok(AtIdentifier::Handle(handle));
    }
    Err(err(format!("Invalid Bluesky actor: {}", id)))
}

/// Convert any serde-Serialize value (a typed AT-Proto record) into the
/// untyped `Unknown` variant the lower-level XRPC create_record API takes,
/// going through JSON to avoid manual `Ipld`/`DataModel` construction.
fn serialize_to_unknown<T: serde::Serialize>(
    value: &T,
) -> Result<atrium_api::types::Unknown, MastodonError> {
    let json = serde_json::to_value(value)
        .map_err(|e| err(format!("Bluesky serialise failed: {}", e)))?;
    serde_json::from_value::<atrium_api::types::Unknown>(json)
        .map_err(|e| err(format!("Bluesky Unknown decode failed: {}", e)))
}

fn collect_descendants(
    item: &Union<atrium_api::app::bsky::feed::defs::ThreadViewPostRepliesItem>,
    out: &mut Vec<Status>,
) {
    use atrium_api::app::bsky::feed::defs::ThreadViewPostRepliesItem as Item;
    if let Union::Refs(Item::ThreadViewPost(thread)) = item {
        out.push(post_view_to_status(&thread.data.post));
        if let Some(replies) = &thread.data.replies {
            for r in replies {
                collect_descendants(r, out);
            }
        }
    }
}

fn convert_notification(
    n: &atrium_api::app::bsky::notification::list_notifications::Notification,
    subject_lookup: &std::collections::HashMap<String, Status>,
) -> Option<Notification> {
    let data = &n.data;
    let notification_type = match data.reason.as_str() {
        "like" | "starterpack-joined" => NotificationType::Favourite,
        "repost" => NotificationType::Reblog,
        "follow" => NotificationType::Follow,
        "mention" | "reply" | "quote" => NotificationType::Mention,
        _ => NotificationType::Unknown,
    };
    let account = profile_basic_to_account(&actor_profile_view_to_basic(&data.author));

    // For mention/reply/quote, the notification's own URI IS the post; for
    // like/repost the reason_subject points at the affected post.
    let status = match notification_type {
        NotificationType::Mention => subject_lookup.get(&data.uri).cloned().or_else(|| {
            data.reason_subject
                .as_ref()
                .and_then(|u| subject_lookup.get(u).cloned())
        }),
        NotificationType::Favourite | NotificationType::Reblog => data
            .reason_subject
            .as_ref()
            .and_then(|u| subject_lookup.get(u).cloned()),
        _ => None,
    };

    Some(Notification {
        id: data.uri.clone(),
        notification_type,
        created_at: chrono::DateTime::parse_from_rfc3339(data.indexed_at.as_str())
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now),
        account,
        status,
    })
}

fn actor_profile_view_to_basic(
    profile: &atrium_api::app::bsky::actor::defs::ProfileView,
) -> atrium_api::app::bsky::actor::defs::ProfileViewBasic {
    use atrium_api::app::bsky::actor::defs::ProfileViewBasicData;
    let data = &profile.data;
    ProfileViewBasicData {
        associated: data.associated.clone(),
        avatar: data.avatar.clone(),
        created_at: data.created_at.clone(),
        did: data.did.clone(),
        display_name: data.display_name.clone(),
        handle: data.handle.clone(),
        labels: data.labels.clone(),
        pronouns: data.pronouns.clone(),
        status: data.status.clone(),
        verification: data.verification.clone(),
        viewer: data.viewer.clone(),
    }
    .into()
}

fn actor_view_to_basic(
    profile: &atrium_api::app::bsky::actor::defs::ProfileView,
) -> atrium_api::app::bsky::actor::defs::ProfileViewBasic {
    actor_profile_view_to_basic(profile)
}

fn html_to_plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[allow(dead_code)]
fn _unused_text_to_html(s: &str) -> String {
    text_to_html(s)
}

#[allow(dead_code)]
fn _unused_app_host() -> &'static str {
    BSKY_APP_HOST
}
