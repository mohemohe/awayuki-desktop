use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::account::Account;
use crate::mastodon::types::search::SearchResult;
use crate::mastodon::types::status::Status;

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

    /// Resolve a remote status URI on this server. Used in unified-timeline
    /// mode when the active account differs from the account that fetched
    /// the post: actions like boost/favourite need the status's local id on
    /// the active account's server.
    /// GET /api/v2/search?type=statuses&q=<URI>&resolve=true&limit=1
    pub async fn lookup_status_by_uri(&self, uri: &str) -> Result<Option<Status>, MastodonError> {
        let query_params = [
            ("type", "statuses"),
            ("q", uri),
            ("resolve", "true"),
            ("limit", "1"),
        ];
        let result: SearchResult = self.get_with_query("/api/v2/search", &query_params).await?;
        Ok(result.statuses.into_iter().next())
    }
}
