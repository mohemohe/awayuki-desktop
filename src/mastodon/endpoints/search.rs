use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::Account;
use crate::mastodon::types::search::SearchResult;

impl MastodonClient {
    /// Search accounts for @mention autocomplete.
    /// GET /api/v1/accounts/search?q=<query>&resolve=false&limit=<limit>
    pub async fn search_accounts(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Account>, MastodonError> {
        let limit_str = limit.to_string();
        let query_params = [("q", query), ("resolve", "false"), ("limit", &limit_str)];
        self.get_with_query("/api/v1/accounts/search", &query_params)
            .await
    }

    /// Search hashtags for #hashtag autocomplete.
    /// GET /api/v2/search?type=hashtags&q=<query>&resolve=false&limit=<limit>&exclude_unreviewed=true
    pub async fn search_hashtags(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<SearchResult, MastodonError> {
        let limit_str = limit.to_string();
        let query_params = [
            ("type", "hashtags"),
            ("q", query),
            ("resolve", "false"),
            ("limit", &limit_str),
            ("exclude_unreviewed", "true"),
        ];
        self.get_with_query("/api/v2/search", &query_params).await
    }
}
