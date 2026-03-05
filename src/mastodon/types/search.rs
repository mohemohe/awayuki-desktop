use serde::{Deserialize, Serialize};

use super::account::Account;
use super::status::{Status, Tag};

/// Response from GET /api/v2/search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub statuses: Vec<Status>,
    #[serde(default)]
    pub hashtags: Vec<Tag>,
}
