use std::future::Future;
use std::time::Instant;

use reqwest::{Client, RequestBuilder, Response, StatusCode};
use url::Url;

use crate::api::http::{
    api_client, body_bytes_limited, body_text_limited, MAX_API_RESPONSE_BYTES,
    MAX_ERROR_RESPONSE_BYTES,
};
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
        let http = api_client()?;

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

    pub async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, MastodonError> {
        self.post_json_idempotent(path, body, None).await
    }

    pub async fn post_json_idempotent<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
        idempotency_key: Option<&str>,
    ) -> Result<T, MastodonError> {
        let request = self.post_json_request(path, body, idempotency_key);
        self.request_with_log("POST", path, request.send(), Self::handle_response)
            .await
    }

    fn post_json_request<B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
        idempotency_key: Option<&str>,
    ) -> RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let request = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(body);
        match idempotency_key {
            Some(key) => request.header("Idempotency-Key", key),
            None => request,
        }
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
                let body = body_bytes_limited(response, MAX_API_RESPONSE_BYTES)
                    .await
                    .map_err(|error| MastodonError::Other(error.to_string()))?;
                serde_json::from_slice(&body).map_err(MastodonError::Json)
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
                let message = body_text_limited(response, MAX_ERROR_RESPONSE_BYTES)
                    .await
                    .unwrap_or_else(|error| error.to_string());
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
            let message = body_text_limited(response, MAX_ERROR_RESPONSE_BYTES)
                .await
                .unwrap_or_else(|error| error.to_string());
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
        let http = api_client()?;
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

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::MastodonClient;

    #[derive(Serialize)]
    struct TestBody {
        status: &'static str,
    }

    #[test]
    fn idempotent_post_uses_header_without_serializing_transport_metadata() {
        let client = MastodonClient::new(
            "example.test",
            "secret-token".to_string(),
            "wss://example.test".to_string(),
        )
        .unwrap();
        let request = client
            .post_json_request(
                "/api/v1/statuses",
                &TestBody { status: "hello" },
                Some("018fba3a-d411-7d8b-9a8d-f2f292cf79e0"),
            )
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get("Idempotency-Key")
                .and_then(|value| value.to_str().ok()),
            Some("018fba3a-d411-7d8b-9a8d-f2f292cf79e0")
        );
        assert_eq!(
            request.body().and_then(|body| body.as_bytes()),
            Some(br#"{"status":"hello"}"#.as_slice())
        );
    }

    #[test]
    fn ordinary_post_does_not_send_an_idempotency_header() {
        let client = MastodonClient::new(
            "example.test",
            "secret-token".to_string(),
            "wss://example.test".to_string(),
        )
        .unwrap();
        let request = client
            .post_json_request(
                "/api/v1/statuses/1/favourite",
                &TestBody { status: "hello" },
                None,
            )
            .build()
            .unwrap();

        assert!(request.headers().get("Idempotency-Key").is_none());
    }
}
