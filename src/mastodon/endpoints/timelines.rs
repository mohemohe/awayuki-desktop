use crate::mastodon::client::{MastodonClient, PaginatedResponse};
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::Status;

/// Pagination parameters for timeline requests
pub struct TimelineParams {
    pub max_id: Option<String>,
    pub since_id: Option<String>,
    pub min_id: Option<String>,
    pub limit: Option<u32>,
}

impl Default for TimelineParams {
    fn default() -> Self {
        Self {
            max_id: None,
            since_id: None,
            min_id: None,
            limit: Some(40),
        }
    }
}

impl TimelineParams {
    fn to_query(&self) -> Vec<(&str, String)> {
        let mut params = Vec::new();
        if let Some(ref id) = self.max_id {
            params.push(("max_id", id.clone()));
        }
        if let Some(ref id) = self.since_id {
            params.push(("since_id", id.clone()));
        }
        if let Some(ref id) = self.min_id {
            params.push(("min_id", id.clone()));
        }
        if let Some(limit) = self.limit {
            params.push(("limit", limit.to_string()));
        }
        params
    }
}

impl MastodonClient {
    pub async fn get_home_timeline(
        &self,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let owned = params.to_query();
        let query: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.get_with_query("/api/v1/timelines/home", &query).await
    }

    pub async fn get_public_timeline(
        &self,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let mut query = params.to_query();
        if local {
            query.push(("local", "true".to_string()));
        }
        let query_refs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.get_with_query("/api/v1/timelines/public", &query_refs)
            .await
    }

    pub async fn get_list_timeline(
        &self,
        list_id: &str,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let owned = params.to_query();
        let query: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let path = format!("/api/v1/timelines/list/{}", list_id);
        self.get_with_query(&path, &query).await
    }

    pub async fn get_hashtag_timeline(
        &self,
        tag: &str,
        local: bool,
        params: &TimelineParams,
    ) -> Result<Vec<Status>, MastodonError> {
        let mut query = params.to_query();
        if local {
            query.push(("local", "true".to_string()));
        }
        let query_refs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let path = format!("/api/v1/timelines/tag/{}", tag);
        self.get_with_query(&path, &query_refs).await
    }

    pub async fn get_bookmarks(
        &self,
        params: &TimelineParams,
    ) -> Result<PaginatedResponse<Vec<Status>>, MastodonError> {
        let owned = params.to_query();
        let query: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.get_with_query_paginated("/api/v1/bookmarks", &query)
            .await
    }
}
