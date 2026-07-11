use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use reqwest::{Client, Response};
use tokio_util::io::ReaderStream;

use crate::constants::APP_USER_AGENT;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const API_REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
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

pub fn api_client() -> Result<Client, reqwest::Error> {
    client_builder(API_REQUEST_TIMEOUT).build()
}

pub fn download_client() -> Result<Client, reqwest::Error> {
    client_builder(DOWNLOAD_REQUEST_TIMEOUT).build()
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
    use super::*;

    #[test]
    fn clients_have_static_valid_policies() {
        api_client().expect("build API client");
        download_client().expect("build download client");
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
