use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub acct: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub note: String,
    pub url: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub avatar_static: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub header_static: String,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub bot: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub followers_count: i64,
    #[serde(default)]
    pub following_count: i64,
    #[serde(default)]
    pub statuses_count: i64,
    #[serde(default)]
    pub fields: Vec<AccountField>,
    #[serde(default)]
    pub emojis: Vec<CustomEmoji>,
    // Pleroma/Akkoma extension
    #[serde(default)]
    pub pleroma: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountField {
    pub name: String,
    pub value: String,
    pub verified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomEmoji {
    pub shortcode: String,
    pub url: String,
    pub static_url: String,
    #[serde(default)]
    pub visible_in_picker: bool,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub id: String,
    #[serde(default)]
    pub following: bool,
    #[serde(default)]
    pub followed_by: bool,
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub blocked_by: bool,
    #[serde(default)]
    pub muting: bool,
    #[serde(default)]
    pub requested: bool,
    #[serde(default)]
    pub note: String,
}
