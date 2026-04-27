use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::note::MisskeyNote;
use super::user::MisskeyUser;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNotification {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub r#type: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub user: Option<MisskeyUser>,
    #[serde(default)]
    pub note: Option<MisskeyNote>,
    #[serde(default)]
    pub reaction: Option<String>,
}
