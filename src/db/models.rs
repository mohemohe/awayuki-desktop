use sqlx::FromRow;

use crate::mastodon::types::account::Account;
use crate::mastodon::types::status::Status;

#[derive(Debug, FromRow)]
pub struct DbServer {
    pub domain: String,
    pub streaming_url: String,
    pub version: Option<String>,
    pub max_characters: Option<i32>,
    pub instance_json: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DbAccount {
    pub id: String,
    pub server_domain: String,
    pub username: String,
    pub acct: String,
    pub display_name: String,
    pub note: String,
    pub avatar: String,
    pub avatar_static: String,
    pub header: String,
    pub locked: bool,
    pub bot: bool,
    pub followers_count: i64,
    pub following_count: i64,
    pub statuses_count: i64,
    pub created_at: String,
    pub fetched_at: String,
    pub fields_json: Option<String>,
    pub emojis_json: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct DbStatus {
    pub id: String,
    pub server_domain: String,
    pub uri: String,
    pub url: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub account_id: String,
    pub content: String,
    pub visibility: String,
    pub sensitive: bool,
    pub spoiler_text: String,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub replies_count: i64,
    pub in_reply_to_id: Option<String>,
    pub in_reply_to_account_id: Option<String>,
    pub reblog_of_id: Option<String>,
    pub language: Option<String>,
    pub pinned: Option<bool>,
    pub favourited: Option<bool>,
    pub reblogged: Option<bool>,
    pub muted: Option<bool>,
    pub bookmarked: Option<bool>,
    pub poll_json: Option<String>,
    pub card_json: Option<String>,
    pub mentions_json: Option<String>,
    pub tags_json: Option<String>,
    pub emojis_json: Option<String>,
    pub media_attachments_json: Option<String>,
    pub fetched_at: String,
    pub quote_id: Option<String>,
    pub quote_original_url: Option<String>,
}

#[derive(Debug, FromRow)]
pub struct DbTimelineEntry {
    pub id: i64,
    pub timeline_type: String,
    pub server_domain: String,
    pub status_id: String,
    pub account_acct: String,
    pub position_at: String,
}

#[derive(Debug, FromRow)]
pub struct DbNotification {
    pub id: String,
    pub server_domain: String,
    pub notification_type: String,
    pub created_at: String,
    pub account_id: String,
    pub status_id: Option<String>,
    pub read_at: Option<String>,
    pub fetched_at: String,
}

#[derive(Debug, FromRow)]
pub struct DbLoginAccount {
    pub acct: String,
    pub server_domain: String,
    pub account_id: String,
    pub display_name: String,
    pub avatar: String,
    pub is_active: bool,
    pub access_token: String,
}

#[derive(Debug, FromRow)]
pub struct DbColumnConfig {
    pub id: String,
    pub account_acct: String,
    pub column_type: String,
    pub column_param: Option<String>,
    pub position: i32,
    pub width: Option<i32>,
    pub created_at: String,
    pub name: Option<String>,
    pub max_statuses: Option<i32>,
    pub pane_index: Option<i32>,
}

// Conversion: API Account -> DB params
impl DbAccount {
    pub fn from_api(account: &Account, server_domain: &str) -> Self {
        Self {
            id: account.id.clone(),
            server_domain: server_domain.to_string(),
            username: account.username.clone(),
            acct: account.acct.clone(),
            display_name: account.display_name.clone(),
            note: account.note.clone(),
            avatar: account.avatar.clone(),
            avatar_static: account.avatar_static.clone(),
            header: account.header.clone(),
            locked: account.locked,
            bot: account.bot,
            followers_count: account.followers_count,
            following_count: account.following_count,
            statuses_count: account.statuses_count,
            created_at: account.created_at.to_rfc3339(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
            fields_json: if account.fields.is_empty() {
                None
            } else {
                serde_json::to_string(&account.fields).ok()
            },
            emojis_json: if account.emojis.is_empty() {
                None
            } else {
                serde_json::to_string(&account.emojis).ok()
            },
        }
    }
}

// Conversion: API Status -> DB params
impl DbStatus {
    pub fn from_api(status: &Status, server_domain: &str) -> Self {
        Self {
            id: status.id.clone(),
            server_domain: server_domain.to_string(),
            uri: status.uri.clone(),
            url: status.url.clone(),
            created_at: status.created_at.to_rfc3339(),
            edited_at: status.edited_at.map(|d| d.to_rfc3339()),
            account_id: status.account.id.clone(),
            content: status.content.clone(),
            visibility: status.visibility.clone(),
            sensitive: status.sensitive,
            spoiler_text: status.spoiler_text.clone(),
            reblogs_count: status.reblogs_count,
            favourites_count: status.favourites_count,
            replies_count: status.replies_count,
            in_reply_to_id: status.in_reply_to_id.clone(),
            in_reply_to_account_id: status.in_reply_to_account_id.clone(),
            reblog_of_id: status.reblog.as_ref().map(|r| r.id.clone()),
            language: status.language.clone(),
            pinned: status.pinned,
            favourited: status.favourited,
            reblogged: status.reblogged,
            muted: status.muted,
            bookmarked: status.bookmarked,
            poll_json: status.poll.as_ref().and_then(|p| serde_json::to_string(p).ok()),
            card_json: status.card.as_ref().and_then(|c| serde_json::to_string(c).ok()),
            mentions_json: if status.mentions.is_empty() {
                None
            } else {
                serde_json::to_string(&status.mentions).ok()
            },
            tags_json: if status.tags.is_empty() {
                None
            } else {
                serde_json::to_string(&status.tags).ok()
            },
            emojis_json: if status.emojis.is_empty() {
                None
            } else {
                serde_json::to_string(&status.emojis).ok()
            },
            media_attachments_json: if status.media_attachments.is_empty() {
                None
            } else {
                serde_json::to_string(&status.media_attachments).ok()
            },
            fetched_at: chrono::Utc::now().to_rfc3339(),
            quote_id: status.quote_id.clone(),
            quote_original_url: status.quote_original_url.clone(),
        }
    }
}
