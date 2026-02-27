use std::any::type_name;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use gpui::http_client::{self, AsyncBody, HttpClient, Response};

/// A real HTTP client backed by reqwest, for GPUI image loading etc.
/// Holds its own tokio runtime handle so reqwest can work from GPUI's GCD-based executor.
pub struct ReqwestHttpClient {
    client: reqwest::Client,
    _runtime: tokio::runtime::Runtime,
    handle: tokio::runtime::Handle,
}

impl ReqwestHttpClient {
    pub fn new() -> Arc<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for HTTP client");

        let handle = runtime.handle().clone();

        let client = reqwest::Client::builder()
            .user_agent("awayuki/0.1.0")
            .build()
            .expect("Failed to build reqwest client");

        Arc::new(Self {
            client,
            _runtime: runtime,
            handle,
        })
    }
}

impl HttpClient for ReqwestHttpClient {
    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }

    fn user_agent(&self) -> Option<&gpui::http_client::http::HeaderValue> {
        None
    }

    fn send(
        &self,
        req: gpui::http_client::Request<AsyncBody>,
    ) -> BoxFuture<'static, http_client::Result<Response<AsyncBody>>> {
        let (parts, body) = req.into_parts();
        let client = self.client.clone();
        let handle = self.handle.clone();

        async move {
            // Read body bytes outside tokio (AsyncRead works anywhere)
            let body_bytes = {
                use futures::AsyncReadExt;
                let mut buf = Vec::new();
                let mut body = body;
                body.read_to_end(&mut buf).await?;
                buf
            };

            // Spawn the actual HTTP request on the tokio runtime
            let result: Result<Response<AsyncBody>, String> = {
                let join_handle = handle.spawn(async move {
                    let url = parts.uri.to_string();
                    let method =
                        reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
                            .map_err(|e| e.to_string())?;

                    let mut req_builder = client.request(method, &url);

                    for (key, value) in &parts.headers {
                        req_builder =
                            req_builder.header(key.as_str(), value.as_bytes());
                    }

                    if !body_bytes.is_empty() {
                        req_builder = req_builder.body(body_bytes);
                    }

                    let response =
                        req_builder.send().await.map_err(|e| e.to_string())?;

                    let status = response.status().as_u16();
                    let headers = response.headers().clone();
                    let response_bytes =
                        response.bytes().await.map_err(|e| e.to_string())?;

                    let mut builder =
                        gpui::http_client::Response::builder().status(status);

                    for (key, value) in headers.iter() {
                        builder = builder.header(key.as_str(), value.as_bytes());
                    }

                    let http_response = builder
                        .body(AsyncBody::from(response_bytes.to_vec()))
                        .map_err(|e| e.to_string())?;

                    Ok(http_response)
                });

                match join_handle.await {
                    Ok(Ok(resp)) => Ok(resp),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.to_string()),
                }
            };

            result.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e).into())
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&gpui::http_client::Url> {
        None
    }
}
