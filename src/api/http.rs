use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, Url};
use tokio_util::io::ReaderStream;

use crate::constants::APP_USER_AGENT;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const API_REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const PROBE_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub const MAX_API_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_DOWNLOAD_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_MEDIA_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum BoundedBodyError {
    #[error("HTTP response body exceeds the {limit} byte limit")]
    TooLarge { limit: usize },
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("HTTP response body is not valid UTF-8")]
    InvalidUtf8,
}

#[derive(Debug, thiserror::Error)]
pub enum MultipartFileError {
    #[error("media file is empty")]
    Empty,
    #[error("media file exceeds the 256 MiB limit")]
    TooLarge,
    #[error("media file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid media MIME type: {0}")]
    Mime(#[from] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadUrlError {
    #[error("download URL must use HTTP or HTTPS")]
    UnsupportedScheme,
    #[error("download URL must include a host")]
    MissingHost,
    #[error("download URL must not contain credentials")]
    Credentials,
}

pub fn api_client() -> Result<Client, reqwest::Error> {
    client_builder(API_REQUEST_TIMEOUT).build()
}

/// Short-lived server-capability probes use the same redirect, user-agent,
/// connection, TLS and pool policy as normal API traffic, with a smaller
/// whole-request deadline so login cannot stall on an unresponsive host.
pub fn probe_client() -> Result<Client, reqwest::Error> {
    client_builder(PROBE_REQUEST_TIMEOUT).build()
}

pub fn download_client() -> Result<Client, reqwest::Error> {
    client_builder(DOWNLOAD_REQUEST_TIMEOUT)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.stop();
            }
            match validate_download_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(error) => attempt.error(error),
            }
        }))
        .build()
}

/// Applied to the initial media URL and again by the redirect policy. Private
/// and loopback hosts remain supported for self-hosted instances; credentials,
/// hostless targets and non-HTTP schemes are never valid media redirects.
pub fn validate_download_url(url: &Url) -> Result<(), DownloadUrlError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(DownloadUrlError::UnsupportedScheme);
    }
    if url.host().is_none() {
        return Err(DownloadUrlError::MissingHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DownloadUrlError::Credentials);
    }
    Ok(())
}

/// Build a multipart part backed by an async file stream. Metadata is checked
/// before opening the stream and `Content-Length` is fixed, so neither the IPC
/// boundary nor protocol adapter allocates a `Vec` proportional to file size.
pub async fn streaming_multipart_file(
    path: &Path,
    filename: String,
    mime: &str,
) -> Result<reqwest::multipart::Part, MultipartFileError> {
    let metadata = tokio::fs::metadata(path).await?;
    let length = metadata.len();
    if length == 0 {
        return Err(MultipartFileError::Empty);
    }
    if length > MAX_MEDIA_UPLOAD_BYTES {
        return Err(MultipartFileError::TooLarge);
    }
    let file = tokio::fs::File::open(path).await?;
    let stream = ReaderStream::with_capacity(file, 64 * 1024);
    let body = reqwest::Body::wrap_stream(stream);
    Ok(reqwest::multipart::Part::stream_with_length(body, length)
        .file_name(filename)
        .mime_str(mime)?)
}

fn client_builder(request_timeout: Duration) -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(APP_USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .pool_idle_timeout(Duration::from_secs(90))
        .redirect(Policy::limited(5))
}

pub async fn body_bytes_limited(
    response: Response,
    limit: usize,
) -> Result<Vec<u8>, BoundedBodyError> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > limit as u64)
    {
        return Err(BoundedBodyError::TooLarge { limit });
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(limit as u64) as usize,
    );
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(BoundedBodyError::TooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub async fn body_text_limited(
    response: Response,
    limit: usize,
) -> Result<String, BoundedBodyError> {
    String::from_utf8(body_bytes_limited(response, limit).await?)
        .map_err(|_| BoundedBodyError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn clients_have_static_valid_policies() {
        api_client().expect("build API client");
        probe_client().expect("build probe client");
        download_client().expect("build download client");
    }

    #[test]
    fn download_url_policy_rejects_unsafe_redirect_targets() {
        assert!(
            validate_download_url(&Url::parse("https://cdn.example/media.png").unwrap()).is_ok()
        );
        assert!(validate_download_url(&Url::parse("http://127.0.0.1/media.png").unwrap()).is_ok());
        assert!(matches!(
            validate_download_url(&Url::parse("file:///tmp/secret").unwrap()),
            Err(DownloadUrlError::UnsupportedScheme)
        ));
        assert!(matches!(
            validate_download_url(&Url::parse("https://user:pass@example.test/media").unwrap()),
            Err(DownloadUrlError::Credentials)
        ));
    }

    #[tokio::test]
    async fn download_client_never_follows_an_unsafe_redirect_target() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect server");
        let address = listener.local_addr().expect("redirect server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0u8; 2048];
            let count = stream.read(&mut request).await.expect("read request");
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /media"));
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: file:///tmp/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write redirect");
        });

        let response = download_client()
            .expect("download client")
            .get(format!("http://{address}/media"))
            .send()
            .await
            .expect("unsafe non-HTTP location is not followed");
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Url::parse(value).ok())
            .expect("redirect location");
        assert!(matches!(
            validate_download_url(&location),
            Err(DownloadUrlError::UnsupportedScheme)
        ));
        server.await.expect("redirect server task");
    }

    #[tokio::test]
    async fn request_deadline_stops_a_hanging_server() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hanging server");
        let address = listener.local_addr().expect("hanging server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.expect("read request");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let error = client_builder(Duration::from_millis(50))
            .build()
            .expect("build short-deadline client")
            .get(format!("http://{address}/hang"))
            .send()
            .await
            .expect_err("hanging request must time out");

        assert!(error.is_timeout());
        server.abort();
    }

    #[tokio::test]
    async fn bounded_body_rejects_oversized_content_length_before_buffering() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind oversized server");
        let address = listener.local_addr().expect("oversized server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.expect("read request");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1025\r\nConnection: close\r\n\r\n")
                .await
                .expect("write oversized headers");
        });

        let response = api_client()
            .expect("build API client")
            .get(format!("http://{address}/oversized"))
            .send()
            .await
            .expect("receive headers");
        assert!(matches!(
            body_bytes_limited(response, 1024).await,
            Err(BoundedBodyError::TooLarge { limit: 1024 })
        ));
        server.await.expect("oversized server task");
    }

    #[tokio::test]
    async fn request_deadline_also_applies_while_streaming_a_slow_body() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind slow-body server");
        let address = listener.local_addr().expect("slow-body server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.expect("read request");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\na")
                .await
                .expect("write first body byte");
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = stream.write_all(b"b").await;
        });

        let response = client_builder(Duration::from_millis(50))
            .build()
            .expect("build short-deadline client")
            .get(format!("http://{address}/slow"))
            .send()
            .await
            .expect("receive slow-body headers");
        let error = body_bytes_limited(response, 1024)
            .await
            .expect_err("slow body must hit the request deadline");
        assert!(matches!(
            error,
            BoundedBodyError::Request(ref source) if source.is_timeout()
        ));
        server.abort();
    }

    #[tokio::test]
    async fn shared_redirect_policy_stops_a_redirect_loop() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect-loop server");
        let address = listener.local_addr().expect("redirect-loop server address");
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });

        let error = api_client()
            .expect("build API client")
            .get(format!("http://{address}/loop"))
            .send()
            .await
            .expect_err("redirect loop must be rejected");
        assert!(error.is_redirect());
        server.abort();
    }

    #[tokio::test]
    async fn multipart_file_rejects_a_sparse_file_over_the_upload_limit() {
        let path = std::env::temp_dir().join(format!(
            "awayuki-http-upload-{}.mp4",
            uuid::Uuid::new_v4().simple()
        ));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_MEDIA_UPLOAD_BYTES + 1).unwrap();
        drop(file);
        assert!(matches!(
            streaming_multipart_file(&path, "large.mp4".to_string(), "video/mp4").await,
            Err(MultipartFileError::TooLarge)
        ));
        let _ = std::fs::remove_file(path);
    }
}
