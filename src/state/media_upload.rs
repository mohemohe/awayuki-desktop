use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 256 * 1024;
const CAPABILITY_TTL: Duration = Duration::from_secs(30);
const UPLOAD_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, thiserror::Error)]
pub enum MediaUploadError {
    #[error("media file is empty")]
    Empty,
    #[error("media file exceeds the 256 MiB limit")]
    TooLarge,
    #[error("upload chunk exceeds the 256 KiB limit")]
    ChunkTooLarge,
    #[error("media type is not supported")]
    UnsupportedType,
    #[error("media content does not match its declared type")]
    MimeMismatch,
    #[error("upload capability is invalid or expired")]
    InvalidCapability,
    #[error("upload is incomplete")]
    Incomplete,
    #[error("media I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct UploadProgress {
    pub written: u64,
    pub total: u64,
}

pub struct CompletedUpload {
    pub path: PathBuf,
    pub acting_account_acct: String,
}

impl Drop for CompletedUpload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct PendingUpload {
    path: PathBuf,
    file: tokio::fs::File,
    acting_account_acct: String,
    mime_type: String,
    expected: u64,
    written: u64,
    created_at: Instant,
}

struct DropCapability {
    path: PathBuf,
    expires_at: Instant,
}

#[derive(Default)]
pub struct MediaUploadManager {
    pending: Mutex<HashMap<String, PendingUpload>>,
    dropped_paths: std::sync::Mutex<HashMap<PathBuf, Instant>>,
    drop_capabilities: std::sync::Mutex<HashMap<String, DropCapability>>,
}

impl MediaUploadManager {
    pub fn register_dropped_paths(&self, paths: &[PathBuf]) {
        let now = Instant::now();
        let mut allowed = self.dropped_paths.lock().unwrap();
        allowed.retain(|_, expires_at| *expires_at > now);
        for path in paths.iter().take(16) {
            if let Ok(canonical) = std::fs::canonicalize(path) {
                if canonical.is_file() {
                    allowed.insert(canonical, now + CAPABILITY_TTL);
                }
            }
        }
    }

    pub fn claim_dropped_path(&self, path: &Path) -> Result<String, MediaUploadError> {
        let canonical = std::fs::canonicalize(path)?;
        let now = Instant::now();
        let mut allowed = self.dropped_paths.lock().unwrap();
        allowed.retain(|_, expires_at| *expires_at > now);
        let Some(expires_at) = allowed.remove(&canonical) else {
            return Err(MediaUploadError::InvalidCapability);
        };
        let token = Uuid::new_v4().simple().to_string();
        self.drop_capabilities.lock().unwrap().insert(
            token.clone(),
            DropCapability {
                path: canonical,
                expires_at,
            },
        );
        Ok(token)
    }

    pub async fn consume_dropped_path(
        &self,
        token: &str,
        requested_path: &Path,
    ) -> Result<PathBuf, MediaUploadError> {
        let capability = self
            .drop_capabilities
            .lock()
            .unwrap()
            .remove(token)
            .ok_or(MediaUploadError::InvalidCapability)?;
        let canonical = std::fs::canonicalize(requested_path)?;
        if capability.expires_at <= Instant::now() || capability.path != canonical {
            return Err(MediaUploadError::InvalidCapability);
        }
        validate_media_path(&canonical, None).await?;
        Ok(canonical)
    }

    pub async fn begin(
        &self,
        acting_account_acct: String,
        filename: &str,
        mime_type: &str,
        expected: u64,
    ) -> Result<String, MediaUploadError> {
        if expected == 0 {
            return Err(MediaUploadError::Empty);
        }
        if expected > MAX_UPLOAD_BYTES {
            return Err(MediaUploadError::TooLarge);
        }
        let extension = extension_for_mime(mime_type).ok_or(MediaUploadError::UnsupportedType)?;
        if !filename_matches_mime(filename, extension) {
            return Err(MediaUploadError::MimeMismatch);
        }
        self.remove_expired().await;
        let id = Uuid::new_v4().simple().to_string();
        let path = std::env::temp_dir().join(format!("awayuki-upload-{id}.{extension}"));
        let file = create_private_new(&path).await?;
        self.pending.lock().await.insert(
            id.clone(),
            PendingUpload {
                path,
                file,
                acting_account_acct,
                mime_type: mime_type.to_string(),
                expected,
                written: 0,
                created_at: Instant::now(),
            },
        );
        Ok(id)
    }

    pub async fn append(&self, id: &str, chunk: &[u8]) -> Result<UploadProgress, MediaUploadError> {
        if chunk.len() > MAX_CHUNK_BYTES {
            return Err(MediaUploadError::ChunkTooLarge);
        }
        let mut pending = self.pending.lock().await;
        let upload = pending
            .get_mut(id)
            .ok_or(MediaUploadError::InvalidCapability)?;
        let next = upload.written.saturating_add(chunk.len() as u64);
        if next > upload.expected || next > MAX_UPLOAD_BYTES {
            return Err(MediaUploadError::TooLarge);
        }
        upload.file.write_all(chunk).await?;
        upload.written = next;
        Ok(UploadProgress {
            written: next,
            total: upload.expected,
        })
    }

    pub async fn finish(&self, id: &str) -> Result<CompletedUpload, MediaUploadError> {
        let mut upload = self
            .pending
            .lock()
            .await
            .remove(id)
            .ok_or(MediaUploadError::InvalidCapability)?;
        if upload.written != upload.expected {
            let _ = tokio::fs::remove_file(&upload.path).await;
            return Err(MediaUploadError::Incomplete);
        }
        upload.file.flush().await?;
        upload.file.sync_all().await?;
        drop(upload.file);
        validate_media_path(&upload.path, Some(&upload.mime_type)).await?;
        Ok(CompletedUpload {
            path: upload.path,
            acting_account_acct: upload.acting_account_acct,
        })
    }

    pub async fn cancel(&self, id: &str) {
        if let Some(upload) = self.pending.lock().await.remove(id) {
            let _ = tokio::fs::remove_file(upload.path).await;
        }
    }

    pub async fn cancel_account(&self, acting_account_acct: &str) {
        let mut pending = self.pending.lock().await;
        let ids = pending
            .iter()
            .filter_map(|(id, upload)| {
                (upload.acting_account_acct == acting_account_acct).then_some(id.clone())
            })
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(upload) = pending.remove(&id) {
                let _ = tokio::fs::remove_file(upload.path).await;
            }
        }
    }

    async fn remove_expired(&self) {
        let mut pending = self.pending.lock().await;
        let expired = pending
            .iter()
            .filter_map(|(id, upload)| {
                (upload.created_at.elapsed() >= UPLOAD_TTL).then_some(id.clone())
            })
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(upload) = pending.remove(&id) {
                let _ = tokio::fs::remove_file(upload.path).await;
            }
        }
    }
}

async fn validate_media_path(
    path: &Path,
    declared_mime: Option<&str>,
) -> Result<(), MediaUploadError> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() == 0 {
        return Err(MediaUploadError::Empty);
    }
    if metadata.len() > MAX_UPLOAD_BYTES {
        return Err(MediaUploadError::TooLarge);
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut header = [0_u8; 16];
    let read = file.read(&mut header).await?;
    let detected = detect_mime(&header[..read]).ok_or(MediaUploadError::UnsupportedType)?;
    if let Some(declared) = declared_mime {
        if declared != detected {
            return Err(MediaUploadError::MimeMismatch);
        }
    }
    let extension = extension_for_mime(detected).ok_or(MediaUploadError::UnsupportedType)?;
    if !filename_matches_mime(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(""),
        extension,
    ) {
        return Err(MediaUploadError::MimeMismatch);
    }
    Ok(())
}

fn detect_mime(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if header.get(4..8) == Some(b"ftyp") {
        Some("video/mp4")
    } else if header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        Some("video/webm")
    } else if header.starts_with(b"ID3") || header.starts_with(&[0xff, 0xfb]) {
        Some("audio/mpeg")
    } else if header.starts_with(b"OggS") {
        Some("audio/ogg")
    } else {
        None
    }
}

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "audio/mpeg" => Some("mp3"),
        "audio/ogg" => Some("ogg"),
        _ => None,
    }
}

fn filename_matches_mime(filename: &str, extension: &str) -> bool {
    let actual = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    actual == extension || (extension == "jpg" && actual == "jpeg")
}

async fn create_private_new(path: &Path) -> Result<tokio::fs::File, std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    Ok(tokio::fs::File::from_std(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chunked_upload_is_bounded_and_validated() {
        let manager = MediaUploadManager::default();
        let id = manager
            .begin("alice@example.com".into(), "image.png", "image/png", 12)
            .await
            .unwrap();
        manager.append(&id, b"\x89PNG\r\n\x1a\n").await.unwrap();
        manager.append(&id, b"data").await.unwrap();
        let completed = manager.finish(&id).await.unwrap();
        assert_eq!(completed.acting_account_acct, "alice@example.com");
        assert!(completed.path.exists());
        drop(completed);
    }

    #[tokio::test]
    async fn unregistered_local_path_is_rejected() {
        let manager = MediaUploadManager::default();
        assert!(matches!(
            manager.claim_dropped_path(Path::new("/definitely/not/registered")),
            Err(MediaUploadError::Io(_)) | Err(MediaUploadError::InvalidCapability)
        ));
    }

    #[tokio::test]
    async fn oversized_chunk_is_rejected_and_cancel_removes_temp_file() {
        let manager = MediaUploadManager::default();
        let id = manager
            .begin("alice@example.com".into(), "image.png", "image/png", 1)
            .await
            .unwrap();
        assert!(matches!(
            manager.append(&id, &vec![0; MAX_CHUNK_BYTES + 1]).await,
            Err(MediaUploadError::ChunkTooLarge)
        ));
        manager.cancel(&id).await;
        assert!(matches!(
            manager.finish(&id).await,
            Err(MediaUploadError::InvalidCapability)
        ));
    }

    #[tokio::test]
    async fn dropped_path_capability_is_registered_and_consumed_once() {
        let path =
            std::env::temp_dir().join(format!("awayuki-drop-{}.png", Uuid::new_v4().simple()));
        tokio::fs::write(&path, b"\x89PNG\r\n\x1a\ndata")
            .await
            .unwrap();
        let manager = MediaUploadManager::default();
        manager.register_dropped_paths(std::slice::from_ref(&path));
        let capability = manager.claim_dropped_path(&path).unwrap();
        assert!(matches!(
            manager.claim_dropped_path(&path),
            Err(MediaUploadError::InvalidCapability)
        ));
        assert_eq!(
            manager
                .consume_dropped_path(&capability, &path)
                .await
                .unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );
        assert!(matches!(
            manager.consume_dropped_path(&capability, &path).await,
            Err(MediaUploadError::InvalidCapability)
        ));
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn dropped_video_path_is_validated_from_its_registered_path() {
        let path =
            std::env::temp_dir().join(format!("awayuki-drop-{}.mp4", Uuid::new_v4().simple()));
        tokio::fs::write(&path, b"\0\0\0\x18ftypisomvideo")
            .await
            .unwrap();
        let manager = MediaUploadManager::default();
        manager.register_dropped_paths(std::slice::from_ref(&path));

        let capability = manager.claim_dropped_path(&path).unwrap();
        assert_eq!(
            manager
                .consume_dropped_path(&capability, &path)
                .await
                .unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn account_switch_cleanup_cancels_only_that_accounts_uploads() {
        let manager = MediaUploadManager::default();
        let alice = manager
            .begin("alice@example.test".into(), "alice.png", "image/png", 8)
            .await
            .unwrap();
        let bob = manager
            .begin("bob@example.test".into(), "bob.png", "image/png", 8)
            .await
            .unwrap();
        manager.cancel_account("alice@example.test").await;
        assert!(matches!(
            manager.append(&alice, b"123").await,
            Err(MediaUploadError::InvalidCapability)
        ));
        assert!(manager.append(&bob, b"123").await.is_ok());
        manager.cancel(&bob).await;
    }
}
