use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::notification::Notification;

pub struct NotificationParams {
    pub max_id: Option<String>,
    pub since_id: Option<String>,
    pub min_id: Option<String>,
    pub limit: Option<u32>,
    pub exclude_types: Vec<String>,
}

impl Default for NotificationParams {
    fn default() -> Self {
        Self {
            max_id: None,
            since_id: None,
            min_id: None,
            limit: Some(30),
            exclude_types: Vec::new(),
        }
    }
}

impl MastodonClient {
    pub async fn get_notifications(
        &self,
        params: &NotificationParams,
    ) -> Result<Vec<Notification>, MastodonError> {
        let mut query: Vec<(String, String)> = Vec::new();
        if let Some(ref id) = params.max_id {
            query.push(("max_id".to_string(), id.clone()));
        }
        if let Some(ref id) = params.since_id {
            query.push(("since_id".to_string(), id.clone()));
        }
        if let Some(ref id) = params.min_id {
            query.push(("min_id".to_string(), id.clone()));
        }
        if let Some(limit) = params.limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        for t in &params.exclude_types {
            query.push(("exclude_types[]".to_string(), t.clone()));
        }
        let query_refs: Vec<(&str, &str)> = query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        self.get_with_query("/api/v1/notifications", &query_refs)
            .await
    }

    pub async fn get_notification(&self, id: &str) -> Result<Notification, MastodonError> {
        let path = format!("/api/v1/notifications/{}", id);
        self.get(&path).await
    }

    pub async fn dismiss_notification(&self, id: &str) -> Result<(), MastodonError> {
        let path = format!("/api/v1/notifications/{}/dismiss", id);
        let _: serde_json::Value = self.post_empty(&path).await?;
        Ok(())
    }
}
