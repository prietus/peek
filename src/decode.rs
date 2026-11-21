//! Still-image decoding shared by the viewer, the grid thumbnails and
//! one-shot mode. Applies EXIF orientation, and hands formats the `image`
//! crate can't read (AVIF, HEIC/HEIF, JPEG XL) to ImageMagick or ffmpeg.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, ImageDecoder, ImageReader};

use crate::video::TempDir;

/// Formats decoded by an external tool.
const EXTERNAL_EXTS: &[&str] = &["avif", "heic", "heif", "jxl"];

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

pub fn needs_external(path: &Path) -> bool {
    ext_lower(path).is_some_and(|e| EXTERNAL_EXTS.contains(&e.as_str()))
}

fn tool_ok(program: &str, arg: &str) -> bool {
    Command::new(program)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn magick_available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| tool_ok("magick", "-version"))
}

/// True when at least one external decoder is installed.
pub fn external_available() -> bool {
    magick_available() || crate::video::ffmpeg_available()
}

/// Decode `path` as a single still image, upright according to its EXIF
/// orientation.
pub fn open(path: &Path) -> Result<DynamicImage> {
    if needs_external(path) {
        return decode_external(path);
    }
    let mut decoder = ImageReader::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?
        .with_guessed_format()?
        .into_decoder()
        .with_context(|| format!("failed to decode {}", path.display()))?;
    // A broken orientation tag shouldn't stop the image from showing.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Pixel dimensions as displayed (after orientation), read from the header
/// only. None for externally decoded formats or unreadable files.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    if needs_external(path) {
        return None;
    }
    let mut decoder = ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (w, h) = decoder.dimensions();
    use image::metadata::Orientation::*;
    match decoder.orientation().unwrap_or(NoTransforms) {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => Some((h, w)),
        _ => Some((w, h)),
    }
}

fn decode_external(path: &Path) -> Result<DynamicImage> {
    if !external_available() {
        return Err(anyhow!(
            "{} needs ImageMagick (magick) or ffmpeg installed",
            ext_lower(path).unwrap_or_default().to_uppercase()
        ));
    }
    let tmp = TempDir::new("ipeek-decode")?;
    let out = tmp.0.join("frame.png");

    if magick_available() {
        // `[0]` takes the primary image of multi-image containers.
        let status = Command::new("magick")
            .arg(format!("{}[0]", path.display()))
            .arg("-auto-orient")
            .arg(&out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() && out.exists() {
            return Ok(image::open(&out)?);
        }
    }
    if crate::video::ffmpeg_available() {
        let status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-frames:v", "1"])
            .arg(&out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() && out.exists() {
            return Ok(image::open(&out)?);
        }
    }
    Err(anyhow!("external decoder failed for {}", path.display()))
}
