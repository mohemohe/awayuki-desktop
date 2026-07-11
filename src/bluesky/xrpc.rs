//! XRPC client that intercepts every Bluesky response to capture rate-limit
//! headers, then forwards the request unchanged.
//!
//! `bsky-sdk` defaults to the `atrium-xrpc-client::reqwest::ReqwestClient`,
//! but that client throws away response headers as soon as the body is read.
//! We replicate its `HttpClient::send_http` translation between
//! `http::Request<Vec<u8>>` and `reqwest::Request`/`Response`, but stop along
//! the way to read `RateLimit-*` headers and write the parsed snapshot into
//! a shared slot that the UI can poll.
//!
//! The wrapper sits *below* `atrium-api`'s `SessionClient`, which means the
//! requests we see already have their `Authorization`, `atproto-proxy`, and
//! labelers headers attached — we only have to pass them through. Likewise
//! `XrpcClient::base_uri` is consulted only at agent-construction time
//! (`AtpAgent::new` seeds the endpoint store from it); subsequent calls go
//! through the SessionClient's overridden `base_uri`, so we can return a
//! plain stored string without worrying about endpoint rotation.

use std::time::Instant;

use atrium_api::xrpc::http::{Request, Response};
use atrium_api::xrpc::{HttpClient, XrpcClient};

use crate::api::http::{api_client, body_bytes_limited, MAX_API_RESPONSE_BYTES};
use crate::bluesky::rate_limit::{RateLimitSnapshot, RateLimitState};

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// XRPC client that wraps an inner `reqwest::Client` and writes the latest
/// `RateLimit-*` snapshot from each response into the shared `state` slot.
///
/// `Arc` around the reqwest client is unnecessary because `reqwest::Client`
/// already wraps its internal pool in `Arc` and is cheap to clone. We still
/// take it by value here to make the construction site explicit about reuse.
pub struct RateLimitTrackingClient {
    base_uri: String,
    inner: reqwest::Client,
    state: RateLimitState,
}

impl RateLimitTrackingClient {
    /// Build a tracking client with a fresh default `reqwest::Client`. The
    /// first request will populate `state` once Bluesky echoes back the
    /// usual `RateLimit-*` headers.
    pub fn new(base_uri: impl Into<String>, state: RateLimitState) -> Result<Self, reqwest::Error> {
        Ok(Self {
            base_uri: base_uri.into(),
            inner: api_client()?,
            state,
        })
    }
}

impl HttpClient for RateLimitTrackingClient {
    async fn send_http(
        &self,
        request: Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        // Same translation `atrium_xrpc_client::reqwest::ReqwestClient` does
        // — the only thing we add is the rate-limit capture below.
        let method = request.method().as_str().to_string();
        let path = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str().to_string())
            .unwrap_or_else(|| request.uri().path().to_string());
        let started_at = Instant::now();
        tracing::info!(
            backend = "bluesky",
            domain = self.base_uri.as_str(),
            method = method.as_str(),
            path = path.as_str(),
            "[awayuki][tauri-api] start"
        );
        let request: reqwest::Request = match request.try_into() {
            Ok(request) => request,
            Err(error) => {
                tracing::info!(
                    backend = "bluesky",
                    domain = self.base_uri.as_str(),
                    method = method.as_str(),
                    path = path.as_str(),
                    duration_ms = elapsed_ms(started_at),
                    "[awayuki][tauri-api] error building request: {}",
                    error
                );
                return Err(error.into());
            }
        };
        let response = match self.inner.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                tracing::info!(
                    backend = "bluesky",
                    domain = self.base_uri.as_str(),
                    method = method.as_str(),
                    path = path.as_str(),
                    duration_ms = elapsed_ms(started_at),
                    "[awayuki][tauri-api] error sending request: {}",
                    error
                );
                return Err(error.into());
            }
        };
        let status = response.status();
        let headers = response.headers().clone();

        // Capture before we drain the body so a body-read error doesn't lose
        // the snapshot.
        if let Some(snapshot) = RateLimitSnapshot::from_headers(&headers) {
            // `write` only fails if the lock is poisoned. We don't want a
            // poisoned lock to wedge every subsequent Bluesky request, so we
            // log and move on rather than propagate the panic.
            match self.state.write() {
                Ok(mut guard) => *guard = Some(snapshot),
                Err(e) => {
                    tracing::warn!("Bluesky rate-limit lock poisoned, dropping snapshot: {}", e)
                }
            }
        }

        let body = match body_bytes_limited(response, MAX_API_RESPONSE_BYTES).await {
            Ok(body) => body.to_vec(),
            Err(error) => {
                tracing::info!(
                    backend = "bluesky",
                    domain = self.base_uri.as_str(),
                    method = method.as_str(),
                    path = path.as_str(),
                    status = status.as_u16(),
                    duration_ms = elapsed_ms(started_at),
                    "[awayuki][tauri-api] error reading response body: {}",
                    error
                );
                return Err(error.into());
            }
        };

        let mut builder = Response::builder().status(status);
        for (k, v) in headers.iter() {
            builder = builder.header(k, v);
        }
        match builder.body(body) {
            Ok(response) => {
                tracing::info!(
                    backend = "bluesky",
                    domain = self.base_uri.as_str(),
                    method = method.as_str(),
                    path = path.as_str(),
                    status = status.as_u16(),
                    duration_ms = elapsed_ms(started_at),
                    "[awayuki][tauri-api] success"
                );
                Ok(response)
            }
            Err(error) => {
                tracing::info!(
                    backend = "bluesky",
                    domain = self.base_uri.as_str(),
                    method = method.as_str(),
                    path = path.as_str(),
                    status = status.as_u16(),
                    duration_ms = elapsed_ms(started_at),
                    "[awayuki][tauri-api] error building response: {}",
                    error
                );
                Err(error.into())
            }
        }
    }
}

impl XrpcClient for RateLimitTrackingClient {
    fn base_uri(&self) -> String {
        self.base_uri.clone()
    }
}
