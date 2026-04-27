use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyMeta {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub version: String,
    #[serde(default)]
    pub max_note_text_length: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MisskeyEmojiCatalogEntry {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MisskeyEmojisResponse {
    pub emojis: Vec<MisskeyEmojiCatalogEntry>,
}
