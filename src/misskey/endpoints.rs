//! Mastodon-shaped wrappers around Misskey REST endpoints.
//!
//! Each method here exposes the same signature as the corresponding `MastodonClient` method,
//! so the rest of the app can stay agnostic. Internally we hit the appropriate Misskey endpoint
//! and run the result through `crate::misskey::convert`.

use std::path::Path;

use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::statuses::CreateStatusParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::{Account, CustomEmoji, Relationship};
use crate::mastodon::types::list::List;
use crate::mastodon::types::notification::Notification;
use crate::mastodon::types::search::SearchResult;
use crate::mastodon::types::status::{MediaAttachment, Status, StatusContext, StatusSource};
use crate::misskey::client::MisskeyClient;
use crate::misskey::convert::{
    catalog_to_custom_emojis, note_to_status, notification_to_mastodon, user_to_account,
    visibility_to_misskey,
};
use crate::misskey::types::meta::MisskeyEmojisResponse;
use crate::misskey::types::note::MisskeyNote;
use crate::misskey::types::notification::MisskeyNotification;
use crate::misskey::types::user::{MisskeyRelation, MisskeyUser};

fn timeline_query(params: &TimelineParams) -> serde_json::Value {
    let mut body = serde_json::json!({
        "limit": params.limit.unwrap_or(40).min(100),
    });
    if let Some(ref id) = params.max_id {
        body["untilId"] = serde_json::Value::String(id.clone());
    }
    if let Some(ref id) = params.since_id {
        body["sinceId"] = serde_json::Value::String(id.clone());
    }
    if let Some(ref id) = params.min_id {
        body["sinceId"] = serde_json::Value::String(id.clone());
    }
    body
}

impl MisskeyClient {
    pub async fn verify_credentials(&self) -> Result<Account, MastodonError> {
        let user: MisskeyUser = self.post_empty("/api/i").await?;
        Ok(user_to_account(&user, self.domain()))
    }

    pub async fn get_home_timeline(
        &self,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let body = timeline_query(params);
        let notes: Vec<MisskeyNote> = self.post_json("/api/notes/timeline", body).await?;
        Ok(notes.into_iter().map(|n| note_to_status(&n, self.domain())).collect())
    }

    pub async fn get_public_timeline(
        &self,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let path = if local {
            "/api/notes/local-timeline"
        } else {
            "/api/notes/global-timeline"
        };
        let body = timeline_query(params);
        let notes: Vec<MisskeyNote> = self.post_json(path, body).await?;
        Ok(notes.into_iter().map(|n| note_to_status(&n, self.domain())).collect())
    }

    pub async fn get_list_timeline(
        &self,
        list_id: &str,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let mut body = timeline_query(params);
        body["listId"] = serde_json::Value::String(list_id.to_string());
        let notes: Vec<MisskeyNote> = self.post_json("/api/notes/user-list-timeline", body).await?;
        Ok(notes.into_iter().map(|n| note_to_status(&n, self.domain())).collect())
    }

    pub async fn get_hashtag_timeline(
        &self,
        tag: &str,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let mut body = timeline_query(params);
        body["tag"] = serde_json::Value::String(tag.to_string());
        if local {
            body["limit"] = serde_json::Value::from(params.limit.unwrap_or(40).min(100));
        }
        let notes: Vec<MisskeyNote> = self.post_json("/api/notes/search-by-tag", body).await?;
        Ok(notes.into_iter().map(|n| note_to_status(&n, self.domain())).collect())
    }

    pub async fn get_bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<crate::mastodon::client::PaginatedResponse<Vec<Status>>, MastodonError> {
        // Misskey uses i/favorites which returns wrapper {id, createdAt, note}
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct FavoriteEntry {
            #[allow(dead_code)]
            id: String,
            note: MisskeyNote,
        }
        let body = timeline_query(params);
        let entries: Vec<FavoriteEntry> = self.post_json("/api/i/favorites", body).await?;
        let next_max_id = entries.last().map(|e| e.note.id.clone());
        let data: Vec<Status> = entries
            .into_iter()
            .map(|e| note_to_status(&e.note, self.domain()))
            .collect();
        Ok(crate::mastodon::client::PaginatedResponse {
            data,
            next_max_id,
        })
    }

    pub async fn get_status(&self, id: &str) -> Result<Status, MastodonError> {
        let body = serde_json::json!({ "noteId": id });
        let note: MisskeyNote = self.post_json("/api/notes/show", body).await?;
        Ok(note_to_status(&note, self.domain()))
    }

    pub async fn get_status_context(&self, id: &str) -> Result<StatusContext, MastodonError> {
        // Ancestors: notes/conversation. Descendants: notes/children.
        let ancestors_body = serde_json::json!({ "noteId": id, "limit": 30 });
        let ancestors_notes: Vec<MisskeyNote> = self
            .post_json("/api/notes/conversation", ancestors_body)
            .await?;
        let descendants_body = serde_json::json!({ "noteId": id, "limit": 30, "depth": 12 });
        let descendants_notes: Vec<MisskeyNote> = self
            .post_json("/api/notes/children", descendants_body)
            .await?;

        let ancestors = ancestors_notes
            .iter()
            .rev() // Misskey returns newest-first, Mastodon expects oldest-first
            .map(|n| note_to_status(n, self.domain()))
            .collect();
        let descendants = descendants_notes
            .iter()
            .map(|n| note_to_status(n, self.domain()))
            .collect();
        Ok(StatusContext { ancestors, descendants })
    }

    pub async fn get_status_source(&self, id: &str) -> Result<StatusSource, MastodonError> {
        let body = serde_json::json!({ "noteId": id });
        let note: MisskeyNote = self.post_json("/api/notes/show", body).await?;
        Ok(StatusSource {
            id: note.id,
            text: note.text.unwrap_or_default(),
            spoiler_text: note.cw.unwrap_or_default(),
        })
    }

    pub async fn create_status(&self, params: &CreateStatusParams) -> Result<Status, MastodonError> {
        let mut body = serde_json::json!({});
        if let Some(ref text) = params.status {
            body["text"] = serde_json::Value::String(text.clone());
        }
        if let Some(ref id) = params.in_reply_to_id {
            body["replyId"] = serde_json::Value::String(id.clone());
        }
        if let Some(ref id) = params.quote_id {
            body["renoteId"] = serde_json::Value::String(id.clone());
        }
        if let Some(ref ids) = params.media_ids {
            body["fileIds"] = serde_json::Value::Array(
                ids.iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            );
        }
        if let Some(sensitive) = params.sensitive {
            body["cw"] = if let Some(ref cw) = params.spoiler_text {
                serde_json::Value::String(cw.clone())
            } else if sensitive {
                serde_json::Value::String(String::new())
            } else {
                serde_json::Value::Null
            };
        } else if let Some(ref cw) = params.spoiler_text {
            if !cw.is_empty() {
                body["cw"] = serde_json::Value::String(cw.clone());
            }
        }
        if let Some(ref vis) = params.visibility {
            body["visibility"] = serde_json::Value::String(visibility_to_misskey(vis).to_string());
        }
        if let Some(ref poll) = params.poll {
            body["poll"] = serde_json::json!({
                "choices": poll.options,
                "expiresAt": null,
                "expiredAfter": poll.expires_in * 1000,
                "multiple": poll.multiple.unwrap_or(false),
            });
        }
        #[derive(serde::Deserialize)]
        struct CreateNoteResponse {
            #[serde(rename = "createdNote")]
            created_note: MisskeyNote,
        }
        let resp: CreateNoteResponse = self.post_json("/api/notes/create", body).await?;
        Ok(note_to_status(&resp.created_note, self.domain()))
    }

    pub async fn edit_status(
        &self,
        id: &str,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        // Misskey doesn't support editing in core. We delete + recreate as a fallback,
        // which matches user intent more often than failing outright.
        self.delete_status(id).await?;
        self.create_status(params).await
    }

    pub async fn delete_status(&self, id: &str) -> Result<(), MastodonError> {
        let body = serde_json::json!({ "noteId": id });
        self.post_void("/api/notes/delete", body).await
    }

    pub async fn favourite(&self, id: &str) -> Result<Status, MastodonError> {
        // Map Mastodon "favourite" to Misskey reaction with a star emoji.
        let body = serde_json::json!({ "noteId": id, "reaction": "\u{2b50}" });
        let _: serde_json::Value = self
            .post_json("/api/notes/reactions/create", body)
            .await
            .or_else(|e| match e {
                // "already reacted" — treat as success
                MastodonError::Api { status: 400, .. } => Ok(serde_json::Value::Null),
                other => Err(other),
            })?;
        self.get_status(id).await
    }

    pub async fn unfavourite(&self, id: &str) -> Result<Status, MastodonError> {
        let body = serde_json::json!({ "noteId": id });
        self.post_void("/api/notes/reactions/delete", body).await.ok();
        self.get_status(id).await
    }

    pub async fn reblog(&self, id: &str) -> Result<Status, MastodonError> {
        let body = serde_json::json!({ "renoteId": id });
        #[derive(serde::Deserialize)]
        struct CreateNoteResponse {
            #[serde(rename = "createdNote")]
            created_note: MisskeyNote,
        }
        let resp: CreateNoteResponse = self.post_json("/api/notes/create", body).await?;
        Ok(note_to_status(&resp.created_note, self.domain()))
    }

    pub async fn unreblog(&self, id: &str) -> Result<Status, MastodonError> {
        // Misskey: delete the renote authored by the current user that targets `id`.
        // The simplest correct behaviour is to look up renotes and remove ours.
        let body = serde_json::json!({ "noteId": id, "limit": 100 });
        let renotes: Vec<MisskeyNote> = self
            .post_json("/api/notes/renotes", body)
            .await
            .unwrap_or_default();
        let me: MisskeyUser = self.post_empty("/api/i").await?;
        for renote in renotes {
            if renote.user.id == me.id {
                let body = serde_json::json!({ "noteId": renote.id });
                let _ = self.post_void("/api/notes/delete", body).await;
            }
        }
        self.get_status(id).await
    }

    pub async fn bookmark(&self, id: &str) -> Result<Status, MastodonError> {
        let body = serde_json::json!({ "noteId": id });
        // Newer Misskey: notes/favorites/create
        if self
            .post_void("/api/notes/favorites/create", body.clone())
            .await
            .is_ok()
        {
            return self.get_status(id).await;
        }
        // Some forks also expose `i/favorites/create`.
        self.post_void("/api/i/favorites/create", body).await?;
        self.get_status(id).await
    }

    pub async fn unbookmark(&self, id: &str) -> Result<Status, MastodonError> {
        let body = serde_json::json!({ "noteId": id });
        if self
            .post_void("/api/notes/favorites/delete", body.clone())
            .await
            .is_ok()
        {
            return self.get_status(id).await;
        }
        self.post_void("/api/i/favorites/delete", body).await?;
        self.get_status(id).await
    }

    pub async fn get_poll(
        &self,
        _id: &str,
    ) -> Result<crate::mastodon::types::status::Poll, MastodonError> {
        // Misskey embeds poll in note; refetch the note and pull its poll.
        let body = serde_json::json!({ "noteId": _id });
        let note: MisskeyNote = self.post_json("/api/notes/show", body).await?;
        match note.poll.as_ref() {
            Some(poll) => Ok(crate::misskey::convert::poll_to_mastodon_public(poll, &note.id)),
            None => Err(MastodonError::Other("Note has no poll".into())),
        }
    }

    pub async fn vote_poll(
        &self,
        id: &str,
        params: &crate::mastodon::endpoints::statuses::VotePollParams,
    ) -> Result<crate::mastodon::types::status::Poll, MastodonError> {
        for choice in &params.choices {
            let body = serde_json::json!({ "noteId": id, "choice": choice });
            self.post_void("/api/notes/polls/vote", body).await?;
        }
        self.get_poll(id).await
    }

    pub async fn get_notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, MastodonError> {
        let mut body = serde_json::json!({
            "limit": params.limit.unwrap_or(30).min(100),
            "markAsRead": false,
        });
        if let Some(ref id) = params.max_id {
            body["untilId"] = serde_json::Value::String(id.clone());
        }
        if let Some(ref id) = params.since_id {
            body["sinceId"] = serde_json::Value::String(id.clone());
        }
        if let Some(ref id) = params.min_id {
            body["sinceId"] = serde_json::Value::String(id.clone());
        }
        let notifs: Vec<MisskeyNotification> =
            self.post_json("/api/i/notifications", body).await?;
        Ok(notifs
            .iter()
            .filter_map(|n| notification_to_mastodon(n, self.domain()))
            .collect())
    }

    pub async fn get_notification(&self, _id: &str) -> Result<Notification, MastodonError> {
        Err(MastodonError::Other(
            "Misskey does not expose a single-notification endpoint".into(),
        ))
    }

    pub async fn dismiss_notification(&self, _id: &str) -> Result<(), MastodonError> {
        // Misskey marks notifications read by id-set; close enough to noop for our use case.
        let body = serde_json::json!({});
        self.post_void("/api/notifications/mark-all-as-read", body).await
    }

    pub async fn get_account(&self, id: &str) -> Result<Account, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        let user: MisskeyUser = self.post_json("/api/users/show", body).await?;
        Ok(user_to_account(&user, self.domain()))
    }

    pub async fn get_account_statuses(
        &self,
        id: &str,
        params: &crate::mastodon::endpoints::accounts::AccountStatusesParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let mut body = serde_json::json!({
            "userId": id,
            "limit": params.limit.unwrap_or(20).min(100),
            "withReplies": !params.exclude_replies.unwrap_or(false),
            "withRenotes": !params.exclude_reblogs.unwrap_or(false),
        });
        if let Some(ref id) = params.max_id {
            body["untilId"] = serde_json::Value::String(id.clone());
        }
        if params.only_media.unwrap_or(false) {
            body["withFiles"] = serde_json::Value::Bool(true);
        }
        let notes: Vec<MisskeyNote> = self.post_json("/api/users/notes", body).await?;
        Ok(notes.into_iter().map(|n| note_to_status(&n, self.domain())).collect())
    }

    pub async fn get_relationships(
        &self,
        ids: &[&str],
    ) -> Result<Vec<Relationship>, MastodonError> {
        let body = serde_json::json!({
            "userId": ids.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        });
        let relations: Vec<MisskeyRelation> = self
            .post_json("/api/users/relation", body)
            .await
            .unwrap_or_default();
        Ok(relations
            .into_iter()
            .map(|r| Relationship {
                id: r.id,
                following: r.is_following,
                followed_by: r.is_followed,
                blocking: r.is_blocking,
                blocked_by: r.is_blocked,
                muting: r.is_muted,
                requested: r.has_pending_follow_request_from_you,
                note: String::new(),
            })
            .collect())
    }

    pub async fn follow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        self.post_void("/api/following/create", body).await?;
        self.fetch_relationship(id).await
    }

    pub async fn unfollow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        self.post_void("/api/following/delete", body).await?;
        self.fetch_relationship(id).await
    }

    pub async fn mute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        self.post_void("/api/mute/create", body).await?;
        self.fetch_relationship(id).await
    }

    pub async fn unmute_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        self.post_void("/api/mute/delete", body).await?;
        self.fetch_relationship(id).await
    }

    pub async fn block_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        self.post_void("/api/blocking/create", body).await?;
        self.fetch_relationship(id).await
    }

    pub async fn unblock_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let body = serde_json::json!({ "userId": id });
        self.post_void("/api/blocking/delete", body).await?;
        self.fetch_relationship(id).await
    }

    async fn fetch_relationship(&self, id: &str) -> Result<Relationship, MastodonError> {
        let rels = self.get_relationships(&[id]).await?;
        rels.into_iter()
            .next()
            .ok_or_else(|| MastodonError::Other("No relationship returned".into()))
    }

    pub async fn get_lists(&self) -> Result<Vec<List>, MastodonError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct MisskeyList {
            id: String,
            name: String,
        }
        let lists: Vec<MisskeyList> = self.post_empty("/api/users/lists/list").await?;
        Ok(lists
            .into_iter()
            .map(|l| List {
                id: l.id,
                title: l.name,
            })
            .collect())
    }

    pub async fn get_custom_emojis(&self) -> Result<Vec<CustomEmoji>, MastodonError> {
        let resp: MisskeyEmojisResponse =
            self.post_unauthenticated("/api/emojis", serde_json::json!({})).await?;
        Ok(catalog_to_custom_emojis(&resp.emojis))
    }

    pub async fn search_accounts(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Account>, MastodonError> {
        let body = serde_json::json!({
            "query": query,
            "limit": limit.min(100),
        });
        let users: Vec<MisskeyUser> = self.post_json("/api/users/search", body).await?;
        Ok(users.iter().map(|u| user_to_account(u, self.domain())).collect())
    }

    pub async fn search_hashtags(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<SearchResult, MastodonError> {
        let body = serde_json::json!({
            "query": query,
            "limit": limit.min(100),
        });
        let names: Vec<String> = self.post_json("/api/hashtags/search", body).await.unwrap_or_default();
        let hashtags = names
            .into_iter()
            .map(|name| crate::mastodon::types::status::Tag {
                url: format!("https://{}/tags/{}", self.domain(), name),
                name,
            })
            .collect();
        Ok(SearchResult {
            accounts: vec![],
            statuses: vec![],
            hashtags,
        })
    }

    pub async fn upload_media(&self, file_path: &Path) -> Result<MediaAttachment, MastodonError> {
        let file_bytes = tokio::fs::read(file_path)
            .await
            .map_err(|e| MastodonError::Other(format!("Failed to read file: {}", e)))?;

        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload")
            .to_string();

        let mime = mime_from_extension(file_path);

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename)
            .mime_str(&mime)
            .map_err(|e| MastodonError::Other(format!("Invalid MIME type: {}", e)))?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let file: crate::misskey::types::note::MisskeyDriveFile =
            self.post_multipart("/api/drive/files/create", form).await?;

        Ok(MediaAttachment {
            id: file.id.clone(),
            media_type: if file.r#type.starts_with("image/") {
                "image".to_string()
            } else if file.r#type.starts_with("video/") {
                "video".to_string()
            } else if file.r#type.starts_with("audio/") {
                "audio".to_string()
            } else {
                "unknown".to_string()
            },
            url: Some(file.url.clone()),
            preview_url: file.thumbnail_url.or(Some(file.url)),
            remote_url: None,
            description: file.comment,
            blurhash: file.blurhash,
            meta: None,
        })
    }
}

fn mime_from_extension(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("png") => "image/png".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("webp") => "image/webp".to_string(),
        Some("mp4") => "video/mp4".to_string(),
        Some("webm") => "video/webm".to_string(),
        Some("mov") => "video/quicktime".to_string(),
        Some("mp3") => "audio/mpeg".to_string(),
        Some("ogg") => "audio/ogg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}
