use serde::Serialize;

use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::{Poll, Status, StatusContext, StatusSource};

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

    pub async fn get_status_source(&self, id: &str) -> Result<StatusSource, MastodonError> {
        let path = format!("/api/v1/statuses/{}/source", id);
        self.get(&path).await
    }

    pub async fn create_status(
        &self,
        params: &CreateStatusParams,
    ) -> Result<Status, MastodonError> {
        self.post_json("/api/v1/statuses", params).await
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

    pub async fn get_poll(&self, id: &str) -> Result<Poll, MastodonError> {
        let path = format!("/api/v1/polls/{}", id);
        self.get(&path).await
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
