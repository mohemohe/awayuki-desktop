use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Misskey custom emoji embedded in notes / users.
/// Older Misskey versions return `Vec<{name, url}>` while newer ones return a `HashMap<name, url>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MisskeyEmojis {
    Map(std::collections::HashMap<String, String>),
    List(Vec<MisskeyEmojiEntry>),
}

impl MisskeyEmojis {
    pub fn into_pairs(self) -> Vec<(String, String)> {
        match self {
            Self::Map(map) => map.into_iter().collect(),
            Self::List(list) => list.into_iter().map(|e| (e.name, e.url)).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MisskeyEmojiEntry {
    pub name: String,
    pub url: String,
}

/// Misskey user (subset of fields we care about).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub avatar_blurhash: Option<String>,
    #[serde(default)]
    pub banner_url: Option<String>,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub is_locked: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: Vec<MisskeyUserField>,
    #[serde(default)]
    pub followers_count: Option<i64>,
    #[serde(default)]
    pub following_count: Option<i64>,
    #[serde(default)]
    pub notes_count: Option<i64>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub emojis: Option<MisskeyEmojis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MisskeyUserField {
    pub name: String,
    pub value: String,
}

/// Relationship returned by `users/relation`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyRelation {
    pub id: String,
    #[serde(default)]
    pub is_following: bool,
    #[serde(default)]
    pub is_followed: bool,
    #[serde(default)]
    pub is_blocking: bool,
    #[serde(default)]
    pub is_blocked: bool,
    #[serde(default)]
    pub is_muted: bool,
    #[serde(default)]
    pub has_pending_follow_request_from_you: bool,
}
