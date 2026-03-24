use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::account::{Account, CustomEmoji};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub id: String,
    pub uri: String,
    #[serde(default)]
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub edited_at: Option<DateTime<Utc>>,
    pub account: Account,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub spoiler_text: String,
    #[serde(default)]
    pub media_attachments: Vec<MediaAttachment>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub emojis: Vec<CustomEmoji>,
    #[serde(default)]
    pub reblogs_count: i64,
    #[serde(default)]
    pub favourites_count: i64,
    #[serde(default)]
    pub replies_count: i64,
    #[serde(default)]
    pub in_reply_to_id: Option<String>,
    #[serde(default)]
    pub in_reply_to_account_id: Option<String>,
    #[serde(default)]
    pub reblog: Option<Box<Status>>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub favourited: Option<bool>,
    #[serde(default)]
    pub reblogged: Option<bool>,
    #[serde(default)]
    pub muted: Option<bool>,
    #[serde(default)]
    pub bookmarked: Option<bool>,
    #[serde(default)]
    pub poll: Option<Poll>,
    #[serde(default)]
    pub card: Option<Card>,
    #[serde(default)]
    pub application: Option<StatusApplication>,
    // Quote post (Mastodon 4.5+ / Fedibird / Paon etc.)
    #[serde(default)]
    pub quote_id: Option<String>,
    #[serde(default)]
    pub quote: Option<Box<Status>>,
    #[serde(default)]
    pub quote_original_url: Option<String>,
    // Pleroma/Akkoma extension
    #[serde(default)]
    pub pleroma: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    pub id: String,
    #[serde(rename = "type")]
    pub media_type: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub preview_url: Option<String>,
    #[serde(default)]
    pub remote_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub blurhash: Option<String>,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mention {
    pub id: String,
    pub username: String,
    pub acct: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Poll {
    pub id: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expired: bool,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub votes_count: i64,
    #[serde(default)]
    pub voters_count: Option<i64>,
    #[serde(default)]
    pub options: Vec<PollOption>,
    #[serde(default)]
    pub voted: Option<bool>,
    #[serde(default)]
    pub own_votes: Option<Vec<i64>>,
    #[serde(default)]
    pub emojis: Vec<CustomEmoji>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollOption {
    pub title: String,
    #[serde(default)]
    pub votes_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type", default)]
    pub card_type: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub provider_url: Option<String>,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub author_url: Option<String>,
    #[serde(default)]
    pub blurhash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusApplication {
    pub name: String,
    #[serde(default)]
    pub website: Option<String>,
}

/// Source text of a status (for editing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSource {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub spoiler_text: String,
}

/// Status context (ancestors + descendants)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusContext {
    pub ancestors: Vec<Status>,
    pub descendants: Vec<Status>,
}
