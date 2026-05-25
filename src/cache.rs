use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use image::DynamicImage;

/// On-disk cache path for a thumbnail of `image_path`. None when we can't
/// locate a cache dir or read metadata. Includes canonical path + mtime +
/// size + max-side in the key, so any edit to the source invalidates the
/// cached file automatically.
pub fn thumb_cache_path(image_path: &Path, thumb_max: u32) -> Option<PathBuf> {
    let meta = std::fs::metadata(image_path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    let size = meta.len();
    let canonical = std::fs::canonicalize(image_path).ok()?;
    let mut h = DefaultHasher::new();
    canonical.hash(&mut h);
    mtime.hash(&mut h);
    size.hash(&mut h);
    thumb_max.hash(&mut h);
    let key = h.finish();
    let dir = thumb_cache_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join(format!("{key:016x}.png")))
}

pub fn thumb_cache_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("peek").join("thumbs"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".cache").join("peek").join("thumbs"));
    }
    None
}

/// Load the thumbnail for `path` from disk, falling back to decoding the
/// original + downsampling + writing to cache. Cache misses cost the same as
/// before; cache hits skip the full decode of the source image.
/// Videos route through ffmpeg, which writes a representative frame straight
/// to the cache slot so subsequent loads are pure image::open.
pub fn load_or_build_thumb(path: &Path, thumb_max: u32) -> Result<DynamicImage> {
    let cache_path = thumb_cache_path(path, thumb_max);
    if let Some(cp) = cache_path.as_ref() {
        if let Ok(img) = image::open(cp) {
            return Ok(img);
        }
    }

    if crate::video::is_video(path) {
        let cp = cache_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no cache dir available for video thumbnail"))?;
        crate::video::extract_thumb_to(path, cp, thumb_max)?;
        return Ok(image::open(cp)?);
    }

    let img = image::open(path)?;
    let thumb = img.thumbnail(thumb_max, thumb_max);
    if let Some(cp) = cache_path.as_ref() {
        let _ = thumb.save(cp);
    }
    Ok(thumb)
}
