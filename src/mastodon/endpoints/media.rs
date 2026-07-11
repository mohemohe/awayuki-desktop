use std::path::Path;

use crate::mastodon::client::MastodonClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::MediaAttachment;

impl MastodonClient {
    pub async fn upload_media(&self, file_path: &Path) -> Result<MediaAttachment, MastodonError> {
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload")
            .to_string();

        let mime = mime_from_extension(file_path);

        let part = crate::api::http::streaming_multipart_file(file_path, filename, &mime)
            .await
            .map_err(|error| MastodonError::Other(error.to_string()))?;

        let form = reqwest::multipart::Form::new().part("file", part);

        self.post_multipart("/api/v2/media", form).await
    }
}

fn mime_from_extension(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("png") => "image/png".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("webp") => "image/webp".to_string(),
        Some("mp4") => "video/mp4".to_string(),
        Some("webm") => "video/webm".to_string(),
        Some("mov") => "video/quicktime".to_string(),
        Some("mp3") => "audio/mpeg".to_string(),
        Some("ogg") => "audio/ogg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}
