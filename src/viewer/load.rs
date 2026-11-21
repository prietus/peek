use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, DynamicImage};
use image_webp::WebPDecoder as RawWebPDecoder;

pub(super) struct Frame {
    pub(super) img: DynamicImage,
    pub(super) delay: Duration,
}

pub(super) struct Loaded {
    pub(super) path: PathBuf,
    /// Static images: the image itself.
    /// Animated images: a copy of the first frame; used for dimensions and as
    /// the fallback when editing freezes the animation.
    pub(super) img: DynamicImage,
    pub(super) modified: bool,
    /// None for static images, Some for multi-frame animations (GIF for now).
    pub(super) frames: Option<Vec<Frame>>,
    pub(super) frame_idx: usize,
    pub(super) frame_last: Instant,
    /// Previous states, most recent last, for undo.
    pub(super) undo: Vec<UndoState>,
}

/// Snapshot of a `Loaded` taken just before an edit.
pub(super) struct UndoState {
    pub(super) img: DynamicImage,
    pub(super) frames: Option<Vec<Frame>>,
    pub(super) frame_idx: usize,
    pub(super) modified: bool,
}

impl Loaded {
    pub(super) fn current(&self) -> &DynamicImage {
        match self.frames.as_ref() {
            Some(f) => &f[self.frame_idx].img,
            None => &self.img,
        }
    }

    pub(super) fn current_delay(&self) -> Duration {
        self.frames
            .as_ref()
            .map(|f| f[self.frame_idx].delay)
            .unwrap_or(Duration::from_secs(3600))
    }
}

pub(super) fn load_image(path: &Path) -> Result<Loaded> {
    // Videos: extract a short looping preview as a frame sequence and treat
    // it like an animated GIF/WebP. No audio, no seek — just a visual peek.
    if crate::video::is_video(path) {
        let (imgs, delay) = crate::video::extract_preview(path)?;
        let first = imgs[0].clone();
        let frames: Vec<Frame> = imgs.into_iter().map(|img| Frame { img, delay }).collect();
        return Ok(Loaded {
            path: path.to_path_buf(),
            img: first,
            modified: false,
            frames: Some(frames),
            frame_idx: 0,
            frame_last: Instant::now(),
        undo: Vec::new(),
        });
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());

    // GIF / animated WebP: try multi-frame decode. Falls back to still on a
    // single frame or decoder error (e.g. non-animated GIF89a, still WebP).
    let multi = match ext.as_deref() {
        Some("gif") => load_gif_frames(path).ok(),
        Some("webp") => load_webp_frames(path).ok(),
        _ => None,
    };
    if let Some(frames) = multi
        && frames.len() > 1 {
            let img = frames[0].img.clone();
            return Ok(Loaded {
                path: path.to_path_buf(),
                img,
                modified: false,
                frames: Some(frames),
                frame_idx: 0,
                frame_last: Instant::now(),
        undo: Vec::new(),
            });
        }

    let img = image::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    Ok(Loaded {
        path: path.to_path_buf(),
        img,
        modified: false,
        frames: None,
        frame_idx: 0,
        frame_last: Instant::now(),
        undo: Vec::new(),
    })
}

fn load_webp_frames(path: &Path) -> Result<Vec<Frame>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut decoder = RawWebPDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| anyhow::anyhow!("webp decode: {e}"))?;
    if !decoder.is_animated() {
        return Ok(Vec::new());
    }
    let (w, h) = decoder.dimensions();
    let total = decoder.num_frames() as usize;
    let buf_size = decoder
        .output_buffer_size()
        .unwrap_or((w as usize) * (h as usize) * 4);

    let mut frames = Vec::with_capacity(total);
    for _ in 0..total {
        let mut buf = vec![0u8; buf_size];
        let delay_ms = decoder
            .read_frame(&mut buf)
            .map_err(|e| anyhow::anyhow!("webp frame: {e}"))?;
        // image-webp emits RGBA8 for animated frames.
        let img = image::RgbaImage::from_raw(w, h, buf)
            .ok_or_else(|| anyhow::anyhow!("webp frame size mismatch"))?;
        let delay = Duration::from_millis(delay_ms.max(20) as u64);
        frames.push(Frame {
            img: DynamicImage::ImageRgba8(img),
            delay,
        });
    }
    Ok(frames)
}

fn load_gif_frames(path: &Path) -> Result<Vec<Frame>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let decoder = GifDecoder::new(std::io::BufReader::new(file))?;
    let raw_frames = decoder.into_frames().collect_frames()?;

    Ok(raw_frames
        .into_iter()
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            let ms = if d == 0 {
                100.0
            } else {
                n as f64 / d as f64
            };
            // Most browsers clamp GIF frame delays at 20ms (50fps) to keep CPU
            // sane; do the same so 0-delay loops don't pin the redraw thread.
            let delay = Duration::from_millis(ms.round() as u64).max(Duration::from_millis(20));
            Frame {
                img: DynamicImage::ImageRgba8(f.into_buffer()),
                delay,
            }
        })
        .collect())
}
