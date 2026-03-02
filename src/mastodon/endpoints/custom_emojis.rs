use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::CustomEmoji;

impl MastodonClient {
    /// Fetch all custom emojis for the server.
    /// GET /api/v1/custom_emojis
    pub async fn get_custom_emojis(&self) -> Result<Vec<CustomEmoji>, MastodonError> {
        self.get("/api/v1/custom_emojis").await
    }
}
