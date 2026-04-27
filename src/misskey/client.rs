use reqwest::{Client, Response, StatusCode};

use crate::constants::APP_USER_AGENT;
use crate::mastodon::error::MastodonError;

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
        let http = Client::builder().user_agent(APP_USER_AGENT).build()?;
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
        let response = self.http.post(&url).json(&body).send().await?;
        Self::handle_response(response).await
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
        let response = self.http.post(&url).json(&body).send().await?;
        Self::handle_void_response(response).await
    }

    pub async fn post_multipart<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<T, MastodonError> {
        let url = format!("{}{}", self.base_url, path);
        // Misskey accepts `i` either as form field or query param. We use form field.
        let form = form.text("i", self.access_token.clone());
        let response = self.http.post(&url).multipart(form).send().await?;
        Self::handle_response(response).await
    }

    pub(crate) async fn handle_response<T: serde::de::DeserializeOwned>(
        response: Response,
    ) -> Result<T, MastodonError> {
        let status = response.status();
        match status {
            StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED => {
                let body = response.text().await?;
                serde_json::from_str(&body).map_err(MastodonError::Json)
            }
            StatusCode::NO_CONTENT => {
                // Pretend with `null` so empty responses can deserialise to () or Option<_>.
                serde_json::from_str("null").map_err(MastodonError::Json)
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(MastodonError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => Err(MastodonError::RateLimited { retry_after: None }),
            _ => {
                let message = response.text().await.unwrap_or_default();
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
                let message = response.text().await.unwrap_or_default();
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
        let http = Client::builder().user_agent(APP_USER_AGENT).build()?;
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

    #[allow(dead_code)]
    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, MastodonError> {
        let response = self.http.get(url).send().await?;
        MisskeyClient::handle_response(response).await
    }
}
