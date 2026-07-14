use serde::Serialize;

use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::{Poll, Status, StatusContext};

#[derive(Debug, Serialize)]
pub struct CreatePollParams {
    pub options: Vec<String>,
    pub expires_in: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_totals: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct VotePollParams {
    pub choices: Vec<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateStatusParams {
    /// Delivery identifier for providers that support idempotent status
    /// creation. This is transport metadata and must never be serialized into
    /// a provider request body.
    #[serde(skip)]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spoiler_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<CreatePollParams>,
}

impl MastodonClient {
    pub async fn get_status(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}", id);
        self.get(&path).await
    }

    pub async fn get_status_context(&self, id: &str) -> Result<StatusContext, MastodonError> {
        let path = format!("/api/v1/statuses/{}/context", id);
        self.get(&path).await
    }

    pub async fn create_status(
        &self,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        self.post_json_idempotent(
            "/api/v1/statuses",
            params,
            params.idempotency_key.as_deref(),
        )
        .await
    }

    pub async fn edit_status(
        &self,
        id: &str,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}", id);
        self.put_json(&path, params).await
    }

    pub async fn delete_status(&self, id: &str) -> Result<(), MastodonError> {
        let path = format!("/api/v1/statuses/{}", id);
        self.delete(&path).await
    }

    pub async fn favourite(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}/favourite", id);
        self.post_empty(&path).await
    }

    pub async fn unfavourite(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}/unfavourite", id);
        self.post_empty(&path).await
    }

    pub async fn reblog(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}/reblog", id);
        self.post_empty(&path).await
    }

    pub async fn unreblog(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}/unreblog", id);
        self.post_empty(&path).await
    }

    pub async fn bookmark(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}/bookmark", id);
        self.post_empty(&path).await
    }

    pub async fn unbookmark(&self, id: &str) -> Result<Status, MastodonError> {
        let path = format!("/api/v1/statuses/{}/unbookmark", id);
        self.post_empty(&path).await
    }

    pub async fn vote_poll(
        &self,
        id: &str,
        params: &VotePollParams,
    ) -> Result<Poll, MastodonError> {
        let path = format!("/api/v1/polls/{}/votes", id);
        self.post_json(&path, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::CreateStatusParams;

    #[test]
    fn idempotency_key_is_not_part_of_the_provider_body() {
        let value = serde_json::to_value(CreateStatusParams {
            idempotency_key: Some("018fba3a-d411-7d8b-9a8d-f2f292cf79e0".to_string()),
            status: Some("hello".to_string()),
            in_reply_to_id: None,
            media_ids: None,
            sensitive: None,
            spoiler_text: None,
            visibility: None,
            language: None,
            quote_id: None,
            poll: None,
        })
        .unwrap();

        assert_eq!(value, serde_json::json!({ "status": "hello" }));
    }
}
