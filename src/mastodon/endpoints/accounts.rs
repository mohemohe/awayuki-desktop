use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::{Account, Relationship};
use crate::mastodon::types::status::Status;

pub struct AccountStatusesParams {
    pub max_id: Option<String>,
    pub limit: Option<u32>,
    pub pinned: Option<bool>,
    pub exclude_replies: Option<bool>,
}

impl Default for AccountStatusesParams {
    fn default() -> Self {
        Self {
            max_id: None,
            limit: Some(20),
            pinned: None,
            exclude_replies: None,
        }
    }
}

impl AccountStatusesParams {
    fn to_query(&self) -> Vec<(&str, String)> {
        let mut params = Vec::new();
        if let Some(ref id) = self.max_id {
            params.push(("max_id", id.clone()));
        }
        if let Some(limit) = self.limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(true) = self.pinned {
            params.push(("pinned", "true".to_string()));
        }
        if let Some(true) = self.exclude_replies {
            params.push(("exclude_replies", "true".to_string()));
        }
        params
    }
}

impl MastodonClient {
    /// Verify the current user's credentials.
    pub async fn verify_credentials(&self) -> Result<Account, MastodonError> {
        self.get("/api/v1/accounts/verify_credentials").await
    }

    /// Get account by ID.
    pub async fn get_account(&self, id: &str) -> Result<Account, MastodonError> {
        let path = format!("/api/v1/accounts/{}", id);
        self.get(&path).await
    }

    /// Get an account's statuses.
    pub async fn get_account_statuses(
        &self,
        id: &str,
        params: &AccountStatusesParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let owned = params.to_query();
        let query: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let path = format!("/api/v1/accounts/{}/statuses", id);
        self.get_with_query(&path, &query).await
    }

    /// Get relationships with given accounts.
    pub async fn get_relationships(&self, ids: &[&str]) -> Result<Vec<Relationship>, MastodonError> {
        let query: Vec<(&str, &str)> = ids.iter().map(|id| ("id[]", *id)).collect();
        self.get_with_query("/api/v1/accounts/relationships", &query).await
    }

    /// Follow an account.
    pub async fn follow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let path = format!("/api/v1/accounts/{}/follow", id);
        self.post_empty(&path).await
    }

    /// Unfollow an account.
    pub async fn unfollow_account(&self, id: &str) -> Result<Relationship, MastodonError> {
        let path = format!("/api/v1/accounts/{}/unfollow", id);
        self.post_empty(&path).await
    }
}
