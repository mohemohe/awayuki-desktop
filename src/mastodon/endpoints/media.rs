use std::path::Path;

use crate::api::upload_image::{downscale_to_matrix_limit, DownscaledImage};
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

        // The guard owns the temporary downscaled copy until the request ends.
        let downscaled = self.downscale_oversized_image(file_path, &mime).await;
        let upload_path = downscaled
            .as_ref()
            .map_or(file_path, |downscaled| downscaled.path());

        let part = crate::api::http::streaming_multipart_file(upload_path, filename, &mime)
            .await
            .map_err(|error| MastodonError::Other(error.to_string()))?;

        let form = reqwest::multipart::Form::new().part("file", part);

        self.post_multipart("/api/v2/media", form).await
    }

    /// Mastodon rejects images above `image_matrix_limit` pixels with a 422, so
    /// an oversized still image is downscaled to the advertised limit first.
    /// Any failure here falls back to uploading the original file unchanged.
    async fn downscale_oversized_image(
        &self,
        file_path: &Path,
        mime: &str,
    ) -> Option<DownscaledImage> {
        if !matches!(mime, "image/jpeg" | "image/png") {
            return None;
        }
        let matrix_limit = match self.image_matrix_limit().await {
            Ok(limit) => limit?,
            Err(error) => {
                tracing::warn!(
                    domain = self.domain(),
                    "Failed to read the image pixel limit; uploading the original image: {}",
                    error
                );
                return None;
            }
        };
        match downscale_to_matrix_limit(file_path, matrix_limit).await {
            Ok(Some(downscaled)) => {
                let (width, height) = downscaled.dimensions();
                tracing::info!(
                    domain = self.domain(),
                    matrix_limit,
                    width,
                    height,
                    "Downscaled an image above the server pixel limit before upload"
                );
                Some(downscaled)
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    domain = self.domain(),
                    "Failed to downscale an oversized image; uploading the original image: {}",
                    error
                );
                None
            }
        }
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
