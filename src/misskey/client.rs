use std::future::Future;
use std::time::Instant;

use reqwest::{Client, Response, StatusCode};

use crate::api::http::{
    api_client, body_bytes_limited, body_text_limited, MAX_API_RESPONSE_BYTES,
    MAX_ERROR_RESPONSE_BYTES,
};
use crate::mastodon::error::MastodonError;

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// Authenticated client for Misskey REST API.
///
/// All Misskey API endpoints are POST + JSON body. Authentication is performed by adding
/// `i: <token>` to the request body (or via Bearer header on newer servers); we always
/// inject it server-side so callers don't have to remember.
#[derive(Clone)]
pub struct MisskeyClient {
    http: Client,
    pub base_url: String,
    /// `wss://domain` (without trailing path) — `streaming?i=` is appended where needed.
    pub streaming_url: String,
    access_token: String,
    domain: String,
}

impl MisskeyClient {
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
            domain: domain.to_string(),
        })
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Send POST with no body params other than `i`.
    pub async fn post_empty<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, MastodonError> {
        let body = serde_json::json!({ "i": self.access_token });
        self.post_inner(path, body).await
    }

    /// Send POST with arbitrary JSON body. `i` is injected automatically.
    pub async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: serde_json::Value,
    ) -> Result<T, MastodonError> {
        let mut body = params;
        if let serde_json::Value::Object(ref mut map) = body {
            map.insert(
                "i".to_string(),
                serde_json::Value::String(self.access_token.clone()),
            );
        }
        self.post_inner(path, body).await
    }

    /// Send POST without authenticating (for endpoints like /api/meta).
    pub async fn post_unauthenticated<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, MastodonError> {
        self.post_inner(path, body).await
    }

    async fn post_inner<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "POST",
            path,
            self.http.post(&url).json(&body).send(),
            Self::handle_response,
        )
        .await
    }

    /// 204 / empty body endpoints.
    pub async fn post_void(
        &self,
        path: &str,
        params: serde_json::Value,
    ) -> Result<(), MastodonError> {
        let mut body = params;
        if let serde_json::Value::Object(ref mut map) = body {
            map.insert(
                "i".to_string(),
                serde_json::Value::String(self.access_token.clone()),
            );
        }
        let url = format!("{}{}", self.base_url, path);
        self.request_with_log(
            "POST",
            path,
            self.http.post(&url).json(&body).send(),
            Self::handle_void_response,
        )
        .await
    }

    pub async fn post_multipart<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        // Misskey accepts `i` either as form field or query param. We use form field.
        let form = form.text("i", self.access_token.clone());
        self.request_with_log(
            "POST",
            path,
            self.http.post(&url).multipart(form).send(),
            Self::handle_response,
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
            backend = "misskey",
            domain = self.domain(),
            method,
            path,
            "[awayuki][tauri-api] start"
        );
        let response = match send.await {
            Ok(response) => response,
            Err(error) => {
                tracing::info!(
                    backend = "misskey",
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
                backend = "misskey",
                domain = self.domain(),
                method,
                path,
                status,
                duration_ms = elapsed_ms(started_at),
                "[awayuki][tauri-api] success"
            ),
            Err(error) => tracing::info!(
                backend = "misskey",
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

    pub(crate) async fn handle_response<T: serde::de::DeserializeOwned>(
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
            StatusCode::NO_CONTENT => {
                // Pretend with `null` so empty responses can deserialise to () or Option<_>.
                serde_json::from_str("null").map_err(MastodonError::Json)
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(MastodonError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => Err(MastodonError::RateLimited { retry_after: None }),
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

    pub(crate) async fn handle_void_response(response: Response) -> Result<(), MastodonError> {
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(MastodonError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => Err(MastodonError::RateLimited { retry_after: None }),
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
}

/// Helper for unauthenticated probes (instance/meta lookup, miauth check).
pub struct MisskeyUnauthenticatedClient {
    http: Client,
}

impl MisskeyUnauthenticatedClient {
    pub fn new() -> Result<Self, MastodonError> {
        let http = api_client()?;
        Ok(Self { http })
    }

    pub async fn post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<T, MastodonError> {
        let response = self.http.post(url).json(&body).send().await?;
        MisskeyClient::handle_response(response).await
    }
}
