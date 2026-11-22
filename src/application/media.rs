use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::api::http::{
    download_client, media_probe_client, validate_download_url, MAX_DOWNLOAD_BYTES,
};
use crate::application::desktop::RuntimeState;
use crate::ipc::dto::{CancelMediaDownloadRequest, DownloadMediaRequest, ProbeMediaImageRequest};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::observability::OperationContext;
use crate::state::logging;

const MEDIA_DOWNLOAD_PROGRESS_EVENT: &str = "media-download-progress";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaDownloadProgressEvent {
    operation_id: String,
    phase: &'static str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

pub(crate) async fn open_status_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Unsupported URL scheme".to_string());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|error| error.to_string())
}

pub(crate) async fn download_media(
    state: &RuntimeState,
    request: DownloadMediaRequest,
) -> Result<(), AppError> {
    let mut operation =
        OperationContext::start("download_media", request.operation_id.as_deref(), None);
    let parsed = url::Url::parse(&request.url)
        .map_err(|error| operation.finish_error_code(AppErrorCode::Validation, error))?;
    validate_download_url(&parsed)
        .map_err(|error| operation.finish_error_code(AppErrorCode::Validation, error))?;
    let Some(download) = state.media_download_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "download operation is already active",
        ));
    };

    let suggested = request
        .suggested_filename
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| suggested_filename_from_url(&parsed));
    let filename = sanitize_download_filename(&suggested);
    emit_progress(state, operation.id(), "selecting", 0, None).await;
    let chooser = tokio::task::spawn_blocking(move || choose_download_path(&filename));
    let selected = tokio::select! {
        _ = download.token().cancelled() => {
            return Err(operation.finish_error_code(AppErrorCode::Cancelled, "media download cancelled"));
        }
        result = chooser => result
            .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?
            .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?,
    };
    let Some(path) = selected else {
        operation.finish_ok();
        return Ok(());
    };
    let path = unique_download_path(path);
    let parent = path.parent().ok_or_else(|| {
        operation.finish_error_code(
            AppErrorCode::Validation,
            "download path has no parent directory",
        )
    })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;

    operation.phase("api");
    emit_progress(state, operation.id(), "connecting", 0, None).await;
    let client = download_client()
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    let response = tokio::select! {
        _ = download.token().cancelled() => {
            return Err(operation.finish_error_code(AppErrorCode::Cancelled, "media download cancelled"));
        }
        response = client.get(parsed).send() => response.map_err(|error| {
            let code = if error.is_timeout() { AppErrorCode::Timeout } else { AppErrorCode::Internal };
            operation.finish_error_code(code, error)
        })?,
    };
    validate_download_url(response.url())
        .map_err(|error| operation.finish_error_code(AppErrorCode::Validation, error))?;
    let status = response.status();
    if !status.is_success() {
        let code = match status.as_u16() {
            401 | 403 => AppErrorCode::AuthenticationExpired,
            408 | 504 => AppErrorCode::Timeout,
            429 => AppErrorCode::RateLimited,
            _ => AppErrorCode::Internal,
        };
        return Err(
            operation.finish_error_code(code, format!("media download returned HTTP {status}"))
        );
    }
    let total_bytes = response.content_length();
    if total_bytes.is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64) {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            format!(
                "download exceeds the {} MiB size limit",
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            ),
        ));
    }

    let temp_path = parent.join(format!(
        ".{}.awayuki-{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("media"),
        uuid::Uuid::new_v4().simple()
    ));
    let mut temp_guard = TempDownloadGuard::new(temp_path.clone());
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .await
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    let mut downloaded = 0u64;
    let mut last_progress = Instant::now();
    let mut stream = response.bytes_stream();
    emit_progress(state, operation.id(), "downloading", 0, total_bytes).await;
    loop {
        let chunk = tokio::select! {
            _ = download.token().cancelled() => {
                return Err(operation.finish_error_code(AppErrorCode::Cancelled, "media download cancelled"));
            }
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| {
            let code = if error.is_timeout() {
                AppErrorCode::Timeout
            } else {
                AppErrorCode::Internal
            };
            operation.finish_error_code(code, error)
        })?;
        downloaded = downloaded.checked_add(chunk.len() as u64).ok_or_else(|| {
            operation.finish_error_code(AppErrorCode::Internal, "download size overflow")
        })?;
        if downloaded > MAX_DOWNLOAD_BYTES as u64 {
            return Err(operation.finish_error_code(
                AppErrorCode::Validation,
                format!(
                    "download exceeds the {} MiB size limit",
                    MAX_DOWNLOAD_BYTES / (1024 * 1024)
                ),
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
        if last_progress.elapsed() >= Duration::from_millis(100) {
            emit_progress(
                state,
                operation.id(),
                "downloading",
                downloaded,
                total_bytes,
            )
            .await;
            last_progress = Instant::now();
        }
    }
    operation.phase("commit");
    file.flush()
        .await
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    file.sync_all()
        .await
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    drop(file);
    tokio::fs::hard_link(&temp_path, &path)
        .await
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    tokio::fs::remove_file(&temp_path)
        .await
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    temp_guard.disarm();
    emit_progress(state, operation.id(), "completed", downloaded, total_bytes).await;
    operation.finish_ok();
    Ok(())
}

pub(crate) async fn cancel_media_download(
    state: &RuntimeState,
    request: CancelMediaDownloadRequest,
) -> Result<bool, AppError> {
    let mut operation = OperationContext::start(
        "cancel_media_download",
        request.operation_id.as_deref(),
        None,
    );
    if uuid::Uuid::parse_str(&request.target_operation_id).is_err() {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "target operation ID must be a UUID",
        ));
    }
    let cancelled = state
        .media_download_manager()
        .cancel(&request.target_operation_id);
    operation.finish_ok();
    Ok(cancelled)
}

/// Reports whether an opaque video/audio poster URL resolves to an image. Servers
/// such as Mastodon's `/media_proxy/<id>/small` may redirect it to the original
/// video, which WebKit would otherwise decode and animate inside an `<img>`.
pub(crate) async fn probe_media_image(request: ProbeMediaImageRequest) -> Result<bool, AppError> {
    let mut operation =
        OperationContext::start("probe_media_image", request.operation_id.as_deref(), None);
    let parsed = url::Url::parse(&request.url)
        .map_err(|error| operation.finish_error_code(AppErrorCode::Validation, error))?;
    validate_download_url(&parsed)
        .map_err(|error| operation.finish_error_code(AppErrorCode::Validation, error))?;
    let client = shared_media_probe_client()
        .map_err(|error| operation.finish_error_code(AppErrorCode::Internal, error))?;
    operation.phase("api");
    let is_image = response_is_image(client, parsed).await.map_err(|error| {
        let code = if error.is_timeout() {
            AppErrorCode::Timeout
        } else {
            AppErrorCode::Internal
        };
        operation.finish_error_code(code, error.without_url())
    })?;
    operation.finish_ok();
    Ok(is_image)
}

fn shared_media_probe_client() -> Result<&'static reqwest::Client, reqwest::Error> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = media_probe_client()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Dropping the response after its headers abandons the body, so checking a
/// redirected video never downloads the video itself.
async fn response_is_image(
    client: &reqwest::Client,
    url: url::Url,
) -> Result<bool, reqwest::Error> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await?;
    Ok(response.status().is_success()
        && response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(is_image_content_type))
}

fn is_image_content_type(content_type: &str) -> bool {
    content_type
        .trim_start()
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
}

pub(crate) async fn open_log_file() -> Result<(), String> {
    logging::open_in_default_app().map_err(|error| error.to_string())
}

async fn emit_progress(
    state: &RuntimeState,
    operation_id: &str,
    phase: &'static str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) {
    state
        .emit_application_event(
            MEDIA_DOWNLOAD_PROGRESS_EVENT,
            MediaDownloadProgressEvent {
                operation_id: operation_id.to_string(),
                phase,
                downloaded_bytes,
                total_bytes,
            },
            "media download progress",
        )
        .await;
}

struct TempDownloadGuard {
    path: PathBuf,
    armed: bool,
}

impl TempDownloadGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempDownloadGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn unique_download_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("media");
    let extension = path.extension().and_then(|value| value.to_str());
    for suffix in 1..=10_000 {
        let filename = match extension {
            Some(extension) if !extension.is_empty() => {
                format!("{stem} ({suffix}).{extension}")
            }
            _ => format!("{stem} ({suffix})"),
        };
        let candidate = parent.join(filename);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}", uuid::Uuid::new_v4().simple()))
}

fn suggested_filename_from_url(url: &url::Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.trim().is_empty())
        .map(urlencoding::decode)
        .and_then(Result::ok)
        .map(|name| name.into_owned())
        .unwrap_or_else(|| "media".to_string())
}

fn sanitize_download_filename(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();

    if sanitized.is_empty() {
        "media".to_string()
    } else {
        sanitized
    }
}

#[cfg(target_os = "macos")]
fn choose_download_path(default_name: &str) -> Result<Option<PathBuf>, String> {
    let script = format!(
        "POSIX path of (choose file name with prompt \"Save media as\" default name {})",
        applescript_string(default_name)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|error| error.to_string())?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || output.status.code() == Some(1) {
        return Ok(None);
    }
    Err(stderr.trim().to_string())
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(not(target_os = "macos"))]
fn choose_download_path(default_name: &str) -> Result<Option<PathBuf>, String> {
    let directory = dirs::download_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(Some(directory.join(default_name)))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    // The video response advertises a body it never sends, so a probe that tried
    // to read the body would hang until the deadline instead of returning.
    async fn spawn_media_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind media server");
        let address = listener.local_addr().expect("media server address");
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept request");
                tokio::spawn(async move {
                    let mut request = [0u8; 2048];
                    let count = stream.read(&mut request).await.expect("read request");
                    let request = String::from_utf8_lossy(&request[..count]).to_string();
                    assert!(request.to_ascii_lowercase().contains("range: bytes=0-0"));
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let head = match path {
                        "/proxy/video" => "302 Found\r\nLocation: /original.m4v\r\nContent-Length: 0",
                        "/proxy/poster" => "302 Found\r\nLocation: /poster\r\nContent-Length: 0",
                        "/original.m4v" => "206 Partial Content\r\nContent-Type: video/x-m4v\r\nContent-Length: 14528468",
                        "/poster" => "206 Partial Content\r\nContent-Type: IMAGE/PNG\r\nContent-Length: 0",
                        "/untyped" => "200 OK\r\nContent-Length: 0",
                        _ => "404 Not Found\r\nContent-Type: image/png\r\nContent-Length: 0",
                    };
                    stream
                        .write_all(format!("HTTP/1.1 {head}\r\n\r\n").as_bytes())
                        .await
                        .expect("write response");
                    if path == "/original.m4v" {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                });
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn media_probe_accepts_only_successful_image_responses() {
        let base = spawn_media_server().await;
        let client = media_probe_client().expect("probe client");
        for (path, expected) in [
            ("/proxy/poster", true),
            ("/proxy/video", false),
            ("/untyped", false),
            ("/missing", false),
        ] {
            let url = url::Url::parse(&format!("{base}{path}")).expect("probe URL");
            assert_eq!(
                response_is_image(&client, url).await.expect("probe"),
                expected,
                "{path}"
            );
        }
    }

    #[test]
    fn image_content_type_ignores_case_and_parameters() {
        assert!(is_image_content_type("image/png"));
        assert!(is_image_content_type(" Image/WebP; charset=binary"));
        assert!(!is_image_content_type("video/mp4"));
        assert!(!is_image_content_type("image"));
        assert!(!is_image_content_type(""));
    }

    #[test]
    fn download_path_never_reuses_an_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-download-path-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&directory).expect("create download test directory");
        let existing = directory.join("photo.png");
        std::fs::write(&existing, b"existing").expect("write existing download");

        let candidate = unique_download_path(existing.clone());

        assert_eq!(candidate, directory.join("photo (1).png"));
        assert!(!candidate.exists());
        assert_eq!(std::fs::read(existing).expect("read existing"), b"existing");
        std::fs::remove_dir_all(directory).expect("remove download test directory");
    }
}
