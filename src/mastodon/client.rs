use std::future::Future;
use std::time::Instant;

use reqwest::{Client, Response, StatusCode};
use url::Url;

use crate::constants::APP_USER_AGENT;
use crate::mastodon::error::MastodonError;

/// Response with pagination info extracted from Link header
pub struct PaginatedResponse<T> {
    pub data: T,
    pub next_max_id: Option<String>,
}

/// Parse the `max_id` for the next page from an HTTP Link header.
///
/// Link header format: `<https://example.com/api/v1/bookmarks?max_id=2025>; rel="next", ...`
fn parse_next_max_id(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        if !part.contains("rel=\"next\"") {
            continue;
        }
        let url_str = part.split('>').next()?.trim().strip_prefix('<')?;
        let url = Url::parse(url_str).ok()?;
        for (key, value) in url.query_pairs() {
            if key == "max_id" {
                return Some(value.into_owned());
            }
        }
    }
    None
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[derive(Clone)]
pub struct MastodonClient {
    http: Client,
    pub base_url: String,
    pub streaming_url: String,
    access_token: String,
}

impl MastodonClient {
    pub fn new(
        domain: &str,
        access_token: String,
        streaming_url: String,
    ) -> Result<Self, MastodonError> {
        let http = Client::builder().user_agent(APP_USER_AGENT).build()?;

        Ok(Self {
            http,
            base_url: format!("https://{}", domain),
            streaming_url,
            access_token,
        })
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn domain(&self) -> &str {
        self.base_url
            .strip_prefix("https://")
            .unwrap_or(&self.base_url)
    }

    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "GET",
            path,
            self.http.get(&url).bearer_auth(&self.access_token).send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn get_with_query<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "GET",
            path,
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .query(query)
                .send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn get_with_query_paginated<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<PaginatedResponse<T>, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "GET",
            path,
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .query(query)
                .send(),
            Self::handle_response_paginated,
        )
        .await
    }

    pub async fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "POST",
            path,
            self.http
                .post(&url)
                .bearer_auth(&self.access_token)
                .form(form)
                .send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "POST",
            path,
            self.http
                .post(&url)
                .bearer_auth(&self.access_token)
                .json(body)
                .send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn put_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "PUT",
            path,
            self.http
                .put(&url)
                .bearer_auth(&self.access_token)
                .json(body)
                .send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn post_empty<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "POST",
            path,
            self.http.post(&url).bearer_auth(&self.access_token).send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn post_multipart<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "POST",
            path,
            self.http
                .post(&url)
                .bearer_auth(&self.access_token)
                .multipart(form)
                .send(),
            Self::handle_response,
        )
        .await
    }

    pub async fn delete(&self, path: &str) -> Result<(), MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "DELETE",
            path,
            self.http
                .delete(&url)
                .bearer_auth(&self.access_token)
                .send(),
            Self::handle_empty_response,
        )
        .await
    }

    async fn request_with_log<T, SendFut, Handle, HandleFut>(
        &self,
        method: &str,
        path: &str,
        send: SendFut,
        handle: Handle,
    ) -> Result<T, MastodonError>
    where
        SendFut: Future<Output = Result<Response, reqwest::Error>>,
        Handle: FnOnce(Response) -> HandleFut,
        HandleFut: Future<Output = Result<T, MastodonError>>,
    {
        let started_at = Instant::now();
        tracing::info!(
            backend = "mastodon",
            domain = self.domain(),
            method,
            path,
            "[awayuki][tauri-api] start"
        );
        let response = match send.await {
            Ok(response) => response,
            Err(error) => {
                tracing::info!(
                    backend = "mastodon",
                    domain = self.domain(),
                    method,
                    path,
                    duration_ms = elapsed_ms(started_at),
                    "[awayuki][tauri-api] error sending request: {}",
                    error
                );
                return Err(error.into());
            }
        };
        let status = response.status().as_u16();
        let result = handle(response).await;
        match &result {
            Ok(_) => tracing::info!(
                backend = "mastodon",
                domain = self.domain(),
                method,
                path,
                status,
                duration_ms = elapsed_ms(started_at),
                "[awayuki][tauri-api] success"
            ),
            Err(error) => tracing::info!(
                backend = "mastodon",
                domain = self.domain(),
                method,
                path,
                status,
                duration_ms = elapsed_ms(started_at),
                "[awayuki][tauri-api] error handling response: {}",
                error
            ),
        }
        result
    }

    async fn handle_response_paginated<T: serde::de::DeserializeOwned>(
        response: Response,
    ) -> Result<PaginatedResponse<T>, MastodonError> {
        let next_max_id = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_next_max_id);

        let data = Self::handle_response(response).await?;
        Ok(PaginatedResponse { data, next_max_id })
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(
        response: Response,
    ) -> Result<T, MastodonError> {
        let status = response.status();

        match status {
            StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED => {
                let body = response.text().await?;
                serde_json::from_str(&body).map_err(MastodonError::Json)
            }
            StatusCode::UNAUTHORIZED => Err(MastodonError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok());
                Err(MastodonError::RateLimited { retry_after })
            }
            _ => {
                let message = response.text().await.unwrap_or_default();
                Err(MastodonError::Api {
                    status: status.as_u16(),
                    message,
                })
            }
        }
    }

    async fn handle_empty_response(response: Response) -> Result<(), MastodonError> {
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status == StatusCode::UNAUTHORIZED {
            Err(MastodonError::Unauthorized)
        } else {
            let message = response.text().await.unwrap_or_default();
            Err(MastodonError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }
}

/// Unauthenticated HTTP helpers for OAuth flow
pub struct UnauthenticatedClient {
    http: Client,
}

impl UnauthenticatedClient {
    pub fn new() -> Result<Self, MastodonError> {
        let http = Client::builder().user_agent(APP_USER_AGENT).build()?;
        Ok(Self { http })
    }

    pub async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, MastodonError> {
        let response = self.http.get(url).send().await?;
        MastodonClient::handle_response(response).await
    }

    pub async fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, MastodonError> {
        let response = self.http.post(url).form(form).send().await?;
        MastodonClient::handle_response(response).await
    }
}
