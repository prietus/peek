use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Result, anyhow};
use image::DynamicImage;

const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v", "mpeg", "mpg"];
/// Preview animation tuning: ~10 seconds at 8 fps, 480 px max side. Tuned to
/// keep ffmpeg invocations short and RAM bounded even for long videos.
const PREVIEW_FPS: u32 = 8;
const PREVIEW_MAX_FRAMES: u32 = 80;
const PREVIEW_MAX_SIDE: u32 = 480;
/// Frame timestamp used for grid thumbnails. Skips title/black frames at t=0
/// without needing a duration probe.
const THUMB_SEEK_SECS: f32 = 1.0;

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map(|e| VIDEO_EXTS.contains(&e.as_str()))
        .unwrap_or(false)
}

/// True once `ffmpeg -version` exits cleanly. Result is cached for the
/// process lifetime so we don't probe on every grid cell.
pub fn ffmpeg_available() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Extract a single representative frame to `dest` (PNG). `dest` should have
/// a `.png` extension so ffmpeg picks the encoder automatically.
pub fn extract_thumb_to(path: &Path, dest: &Path, max_side: u32) -> Result<()> {
    if !ffmpeg_available() {
        return Err(anyhow!("ffmpeg not found in PATH"));
    }
    // `-ss` BEFORE `-i` enables fast input seeking (keyframe-accurate, but
    // we don't need exact accuracy for a thumb).
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{THUMB_SEEK_SECS:.3}"))
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-vf"])
        .arg(format!("scale='min({max_side},iw)':-1"))
        .arg(dest)
        .stdout(Stdio::null())
        .status()?;
    if !status.success() {
        // The fast seek can land past EOF for very short clips. Retry from t=0.
        let status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf"])
            .arg(format!("scale='min({max_side},iw)':-1"))
            .arg(dest)
            .stdout(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(anyhow!("ffmpeg failed to extract thumb from {}", path.display()));
        }
    }
    Ok(())
}

/// Extract a short looping preview as a flat list of frames + per-frame delay.
/// Returns up to PREVIEW_MAX_FRAMES frames at PREVIEW_FPS, scaled to
/// PREVIEW_MAX_SIDE. The frames live in a temp dir that is cleaned up before
/// returning, so the caller owns no on-disk state.
pub fn extract_preview(path: &Path) -> Result<(Vec<DynamicImage>, Duration)> {
    if !ffmpeg_available() {
        return Err(anyhow!("ffmpeg not found in PATH"));
    }
    let tmp = TempDir::new("peek-video")?;
    let pattern = tmp.0.join("frame-%05d.jpg");

    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-vf"])
        .arg(format!(
            "fps={PREVIEW_FPS},scale='min({PREVIEW_MAX_SIDE},iw)':-1"
        ))
        .args(["-frames:v"])
        .arg(PREVIEW_MAX_FRAMES.to_string())
        .args(["-q:v", "4"]) // JPEG quality 1-31, lower = better. 4 is a good middle.
        .arg(&pattern)
        .stdout(Stdio::null())
        .status()?;

    if !status.success() {
        return Err(anyhow!("ffmpeg failed to extract frames from {}", path.display()));
    }

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&tmp.0)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    let mut frames = Vec::with_capacity(entries.len());
    for f in entries {
        if let Ok(img) = image::open(&f) {
            frames.push(img);
        }
    }

    if frames.is_empty() {
        return Err(anyhow!("no frames extracted from {}", path.display()));
    }

    let delay = Duration::from_secs_f32(1.0 / PREVIEW_FPS as f32);
    Ok((frames, delay))
}

/// RAII temp dir under `$TMPDIR` that wipes itself on drop. Avoids pulling in
/// the `tempfile` crate for a single use site.
struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
