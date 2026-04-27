use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::user::{MisskeyEmojis, MisskeyUser};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNote {
    pub id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub user_id: String,
    pub user: MisskeyUser,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub cw: Option<String>,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub local_only: bool,
    #[serde(default)]
    pub reply_id: Option<String>,
    #[serde(default)]
    pub renote_id: Option<String>,
    #[serde(default)]
    pub reply: Option<Box<MisskeyNote>>,
    #[serde(default)]
    pub renote: Option<Box<MisskeyNote>>,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub files: Vec<MisskeyDriveFile>,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub reactions: serde_json::Value,
    #[serde(default)]
    pub renote_count: i64,
    #[serde(default)]
    pub replies_count: i64,
    #[serde(default)]
    pub my_reaction: Option<String>,
    #[serde(default)]
    pub poll: Option<MisskeyPoll>,
    #[serde(default)]
    pub emojis: Option<MisskeyEmojis>,
    #[serde(default)]
    pub reaction_emojis: Option<MisskeyEmojis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyDriveFile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub r#type: String,
    pub url: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub blurhash: Option<String>,
    #[serde(default)]
    pub is_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyPoll {
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub choices: Vec<MisskeyPollChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyPollChoice {
    pub text: String,
    #[serde(default)]
    pub votes: i64,
    #[serde(default)]
    pub is_voted: bool,
}
