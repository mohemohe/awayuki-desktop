use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::list::List;

impl MastodonClient {
    /// Get all lists belonging to the authenticated user.
    pub async fn get_lists(&self) -> Result<Vec<List>, MastodonError> {
        self.get("/api/v1/lists").await
    }
}
