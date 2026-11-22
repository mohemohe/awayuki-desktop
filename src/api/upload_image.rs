//! Downscales still images that exceed a server's advertised pixel limit.
//!
//! Servers such as Mastodon reject an upload whose `width * height` exceeds
//! `configuration.media_attachments.image_matrix_limit`. The original file is
//! never modified: an oversized JPEG or PNG is re-encoded into a private
//! temporary file that is removed when the returned guard is dropped.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::{JpegDecoder, JpegEncoder};
use image::codecs::png::{PngDecoder, PngEncoder};
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, Limits};
use uuid::Uuid;

/// High enough that the server-side re-encode, not this one, bounds quality.
const JPEG_QUALITY: u8 = 95;

#[derive(Debug, thiserror::Error)]
pub enum DownscaleError {
    #[error("image I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("image processing failed: {0}")]
    Image(#[from] image::ImageError),
    #[error("image processing task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// A downscaled copy in the temporary directory, deleted on drop.
#[derive(Debug)]
pub struct DownscaledImage {
    path: PathBuf,
    width: u32,
    height: u32,
}

impl DownscaledImage {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for DownscaledImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Returns a downscaled copy when `path` is a still JPEG or PNG with more than
/// `matrix_limit` pixels, and `None` when the file can be uploaded as is
/// (already within the limit, animated, or another media type).
///
/// The copy keeps the source format, aspect ratio and ICC profile. The Exif
/// orientation is applied to the pixels because no other Exif data is carried
/// over.
pub async fn downscale_to_matrix_limit(
    path: &Path,
    matrix_limit: u64,
) -> Result<Option<DownscaledImage>, DownscaleError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || downscale_blocking(&path, matrix_limit)).await?
}

fn downscale_blocking(
    path: &Path,
    matrix_limit: u64,
) -> Result<Option<DownscaledImage>, DownscaleError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut header = [0_u8; 16];
    let read = reader.read(&mut header)?;
    reader.rewind()?;

    match image::guess_format(&header[..read]) {
        Ok(ImageFormat::Jpeg) => {
            let mut decoder = JpegDecoder::new(reader)?;
            decoder.set_limits(Limits::default())?;
            downscale_decoder(decoder, matrix_limit, ImageFormat::Jpeg)
        }
        Ok(ImageFormat::Png) => {
            let decoder = PngDecoder::with_limits(reader, Limits::default())?;
            if decoder.is_apng()? {
                return Ok(None);
            }
            downscale_decoder(decoder, matrix_limit, ImageFormat::Png)
        }
        _ => Ok(None),
    }
}

fn downscale_decoder(
    mut decoder: impl ImageDecoder,
    matrix_limit: u64,
    format: ImageFormat,
) -> Result<Option<DownscaledImage>, DownscaleError> {
    let (width, height) = decoder.dimensions();
    let Some((target_width, target_height)) = target_dimensions(width, height, matrix_limit) else {
        return Ok(None);
    };
    // Metadata is best effort: a damaged profile or Exif block must not block
    // an upload whose pixels decode correctly.
    let icc_profile = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().ok();

    let source = DynamicImage::from_decoder(decoder)?;
    let mut resized = source.resize_exact(target_width, target_height, FilterType::Lanczos3);
    drop(source);
    if let Some(orientation) = orientation {
        resized.apply_orientation(orientation);
    }

    let extension = match format {
        ImageFormat::Jpeg => "jpg",
        _ => "png",
    };
    let path = std::env::temp_dir().join(format!(
        "awayuki-upload-resized-{}.{extension}",
        Uuid::new_v4().simple()
    ));
    let file = create_private_new(&path)?;
    let output = DownscaledImage {
        path,
        width: resized.width(),
        height: resized.height(),
    };
    let mut writer = BufWriter::new(file);
    match format {
        ImageFormat::Jpeg => {
            let mut encoder = JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY);
            if let Some(icc_profile) = icc_profile {
                let _ = encoder.set_icc_profile(icc_profile);
            }
            resized.write_with_encoder(encoder)?;
        }
        _ => {
            let mut encoder = PngEncoder::new(&mut writer);
            if let Some(icc_profile) = icc_profile {
                let _ = encoder.set_icc_profile(icc_profile);
            }
            resized.write_with_encoder(encoder)?;
        }
    }
    writer.flush()?;
    Ok(Some(output))
}

/// Largest size with the source aspect ratio whose pixel count fits `limit`,
/// or `None` when the source already fits.
fn target_dimensions(width: u32, height: u32, limit: u64) -> Option<(u32, u32)> {
    let pixels = u64::from(width) * u64::from(height);
    if limit == 0 || pixels <= limit {
        return None;
    }
    let scale = (limit as f64 / pixels as f64).sqrt();
    let target_width = ((f64::from(width) * scale).floor() as u64).clamp(1, limit);
    let target_height = ((f64::from(height) * scale).floor() as u64).clamp(1, limit / target_width);
    Some((target_width as u32, target_height as u32))
}

fn create_private_new(path: &Path) -> Result<File, std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use image::{ImageReader, Rgb, RgbImage};

    use super::*;

    fn temp_path(extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "awayuki-upload-image-test-{}.{extension}",
            Uuid::new_v4().simple()
        ))
    }

    fn write_gradient(path: &Path, width: u32, height: u32) {
        RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        })
        .save(path)
        .unwrap();
    }

    #[test]
    fn target_dimensions_fit_the_limit_and_keep_the_aspect_ratio() {
        assert_eq!(target_dimensions(4096, 4096, 16_777_216), None);
        assert_eq!(target_dimensions(5712, 4284, 0), None);

        let (width, height) = target_dimensions(5712, 4284, 16_777_216).unwrap();
        assert_eq!((width, height), (4729, 3547));
        assert!(u64::from(width) * u64::from(height) <= 16_777_216);

        assert_eq!(target_dimensions(100_000, 1, 1_000), Some((1_000, 1)));
        assert_eq!(target_dimensions(1, 100_000, 1_000), Some((1, 1_000)));
        assert_eq!(target_dimensions(10, 10, 1), Some((1, 1)));
    }

    #[tokio::test]
    async fn oversized_jpeg_and_png_are_downscaled_in_their_own_format() {
        for (extension, format) in [("jpg", ImageFormat::Jpeg), ("png", ImageFormat::Png)] {
            let source = temp_path(extension);
            write_gradient(&source, 200, 100);

            let resized = downscale_to_matrix_limit(&source, 5_000)
                .await
                .unwrap()
                .expect("image over the limit is downscaled");
            assert_eq!(resized.dimensions(), (100, 50));
            let reader = ImageReader::open(resized.path())
                .unwrap()
                .with_guessed_format()
                .unwrap();
            assert_eq!(reader.format(), Some(format));
            assert_eq!(reader.into_dimensions().unwrap(), (100, 50));

            let resized_path = resized.path().to_path_buf();
            drop(resized);
            assert!(!resized_path.exists());
            assert!(source.exists());
            let _ = std::fs::remove_file(source);
        }
    }

    #[tokio::test]
    async fn files_that_fit_or_are_not_still_images_are_left_alone() {
        let image = temp_path("png");
        write_gradient(&image, 20, 10);
        assert!(downscale_to_matrix_limit(&image, 200)
            .await
            .unwrap()
            .is_none());
        let _ = std::fs::remove_file(image);

        let video = temp_path("mp4");
        std::fs::write(&video, b"\0\0\0\x18ftypisomvideo").unwrap();
        assert!(downscale_to_matrix_limit(&video, 1)
            .await
            .unwrap()
            .is_none());
        let _ = std::fs::remove_file(video);
    }
}
