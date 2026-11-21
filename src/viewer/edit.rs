use std::path::Path;

use anyhow::Result;
use image::{DynamicImage, ImageFormat};

/// Step sizes for the brightness/contrast keys and menu entries.
pub(super) const BRIGHTNESS_STEP: i32 = 10;
pub(super) const CONTRAST_STEP: f32 = 10.0;

#[derive(Copy, Clone, Debug)]
pub(super) enum EditOp {
    RotateCw,
    RotateCcw,
    Rotate180,
    FlipH,
    FlipV,
    /// Add this much to every channel (-255..255).
    Brightness(i32),
    /// Contrast change in percent; negative lowers contrast.
    Contrast(f32),
    Grayscale,
    Invert,
    /// Gaussian blur with this sigma.
    Blur(f32),
    Sharpen,
}

impl EditOp {
    pub(super) fn apply(self, img: &DynamicImage) -> DynamicImage {
        match self {
            EditOp::RotateCw => img.rotate90(),
            EditOp::RotateCcw => img.rotate270(),
            EditOp::Rotate180 => img.rotate180(),
            EditOp::FlipH => img.fliph(),
            EditOp::FlipV => img.flipv(),
            EditOp::Brightness(v) => img.brighten(v),
            EditOp::Contrast(c) => img.adjust_contrast(c),
            EditOp::Grayscale => img.grayscale(),
            EditOp::Invert => {
                let mut out = img.clone();
                out.invert();
                out
            }
            EditOp::Blur(sigma) => img.blur(sigma),
            EditOp::Sharpen => img.unsharpen(1.5, 2),
        }
    }

    /// Short past-tense description for the status bar.
    pub(super) fn describe(self) -> String {
        match self {
            EditOp::RotateCw => "rotated 90° CW".into(),
            EditOp::RotateCcw => "rotated 90° CCW".into(),
            EditOp::Rotate180 => "rotated 180°".into(),
            EditOp::FlipH => "flipped horizontally".into(),
            EditOp::FlipV => "flipped vertically".into(),
            EditOp::Brightness(v) => format!("brightness {v:+}"),
            EditOp::Contrast(c) => format!("contrast {c:+}"),
            EditOp::Grayscale => "converted to grayscale".into(),
            EditOp::Invert => "inverted colours".into(),
            EditOp::Blur(_) => "blurred".into(),
            EditOp::Sharpen => "sharpened".into(),
        }
    }

    /// True when the op can change the image dimensions, so zoom/pan must reset.
    pub(super) fn changes_geometry(self) -> bool {
        matches!(self, EditOp::RotateCw | EditOp::RotateCcw | EditOp::Rotate180)
    }
}

pub(super) enum MenuItem {
    Op(&'static str, EditOp),
    /// Opens the resize prompt instead of applying an op directly.
    Resize(&'static str),
    Separator,
}

impl MenuItem {
    pub(super) fn label(&self) -> Option<&'static str> {
        match self {
            MenuItem::Op(l, _) | MenuItem::Resize(l) => Some(l),
            MenuItem::Separator => None,
        }
    }
}

pub(super) const MENU: &[MenuItem] = &[
    MenuItem::Op("Rotate 90° CW      r", EditOp::RotateCw),
    MenuItem::Op("Rotate 90° CCW     R", EditOp::RotateCcw),
    MenuItem::Op("Rotate 180°", EditOp::Rotate180),
    MenuItem::Op("Flip horizontal    f", EditOp::FlipH),
    MenuItem::Op("Flip vertical      F", EditOp::FlipV),
    MenuItem::Separator,
    MenuItem::Op("Brightness +       b", EditOp::Brightness(BRIGHTNESS_STEP)),
    MenuItem::Op("Brightness −       B", EditOp::Brightness(-BRIGHTNESS_STEP)),
    MenuItem::Op("Contrast +         e", EditOp::Contrast(CONTRAST_STEP)),
    MenuItem::Op("Contrast −         E", EditOp::Contrast(-CONTRAST_STEP)),
    MenuItem::Op("Grayscale", EditOp::Grayscale),
    MenuItem::Op("Invert colours", EditOp::Invert),
    MenuItem::Op("Blur", EditOp::Blur(2.0)),
    MenuItem::Op("Sharpen", EditOp::Sharpen),
    MenuItem::Separator,
    MenuItem::Resize("Resize…            z"),
];

/// Parse a resize spec relative to a `w × h` image:
/// - `50%`        scale both sides
/// - `800`        width 800, height keeps aspect
/// - `x600`       height 600, width keeps aspect
/// - `800x600`    fit inside 800×600 keeping aspect
/// - `800x600!`   exactly 800×600
pub(super) fn parse_resize(spec: &str, w: u32, h: u32) -> Option<(u32, u32)> {
    let spec = spec.to_ascii_lowercase().replace('×', "x").replace(char::is_whitespace, "");
    let aspect = w as f64 / h.max(1) as f64;
    let ok = |a: f64, b: f64| -> Option<(u32, u32)> {
        let (a, b) = (a.round(), b.round());
        (a >= 1.0 && b >= 1.0 && a <= 65535.0 && b <= 65535.0).then_some((a as u32, b as u32))
    };
    if let Some(pct) = spec.strip_suffix('%') {
        let f: f64 = pct.parse().ok()?;
        return ok(w as f64 * f / 100.0, h as f64 * f / 100.0);
    }
    let (exact, spec) = match spec.strip_suffix('!') {
        Some(rest) => (true, rest),
        None => (false, spec.as_str()),
    };
    match spec.split_once('x') {
        None => {
            let nw: f64 = spec.parse().ok()?;
            ok(nw, nw / aspect)
        }
        Some(("", hs)) => {
            let nh: f64 = hs.parse().ok()?;
            ok(nh * aspect, nh)
        }
        Some((ws, hs)) => {
            let (bw, bh): (f64, f64) = (ws.parse().ok()?, hs.parse().ok()?);
            if exact {
                ok(bw, bh)
            } else {
                let scale = (bw / w as f64).min(bh / h as f64);
                ok(w as f64 * scale, h as f64 * scale)
            }
        }
    }
}

/// Save `img` to `path`, converting the pixel format when the target format
/// can't store it as-is (e.g. RGBA or 16-bit into JPEG).
pub(super) fn save_image(img: &DynamicImage, path: &Path) -> Result<()> {
    let format = ImageFormat::from_path(path)?;
    let converted;
    let img = match format {
        ImageFormat::Jpeg => {
            converted = if img.color().has_color() {
                DynamicImage::ImageRgb8(img.to_rgb8())
            } else {
                DynamicImage::ImageLuma8(img.to_luma8())
            };
            &converted
        }
        _ => img,
    };
    match img.save_with_format(path, format) {
        Ok(()) => Ok(()),
        // Some encoders only take 8-bit RGB(A); retry once in that form.
        Err(_) if !matches!(img, DynamicImage::ImageRgba8(_)) => {
            Ok(DynamicImage::ImageRgba8(img.to_rgba8()).save_with_format(path, format)?)
        }
        Err(e) => Err(e.into()),
    }
}

#[derive(Copy, Clone, Debug)]
pub(super) struct CropState {
    /// Selection corners as fractions of the image (0.0 .. 1.0). x0/y0 is the
    /// top-left, x1/y1 the bottom-right; kept ordered so apply_crop doesn't
    /// have to care which side moved.
    pub(super) x0: f32,
    pub(super) y0: f32,
    pub(super) x1: f32,
    pub(super) y1: f32,
}

impl CropState {
    pub(super) fn new() -> Self {
        Self {
            x0: 0.25,
            y0: 0.25,
            x1: 0.75,
            y1: 0.75,
        }
    }

    /// Selection in pixels as (x, y, width, height), clamped so it always
    /// lies inside a `w × h` image and is at least 1×1.
    pub(super) fn pixel_rect(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let px = |f: f32, n: u32| ((f.clamp(0.0, 1.0) * n as f32).round() as u32).min(n.saturating_sub(1));
        let x0 = px(self.x0.min(self.x1), w);
        let y0 = px(self.y0.min(self.y1), h);
        let x1 = (self.x0.max(self.x1).clamp(0.0, 1.0) * w as f32).round() as u32;
        let y1 = (self.y0.max(self.y1).clamp(0.0, 1.0) * h as f32).round() as u32;
        let cw = x1.saturating_sub(x0).max(1).min(w - x0);
        let ch = y1.saturating_sub(y0).max(1).min(h - y0);
        (x0, y0, cw, ch)
    }
}

pub(super) fn first_op_index() -> usize {
    MENU.iter()
        .position(|item| item.label().is_some())
        .unwrap_or(0)
}

pub(super) fn next_op_index(current: usize, delta: i32) -> usize {
    let len = MENU.len() as i32;
    let mut i = current as i32;
    loop {
        i += delta;
        if i < 0 {
            i = len - 1;
        } else if i >= len {
            i = 0;
        }
        if MENU[i as usize].label().is_some() {
            return i as usize;
        }
        if i == current as i32 {
            return current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_rect_default_is_centered_half() {
        assert_eq!(CropState::new().pixel_rect(100, 200), (25, 50, 50, 100));
    }

    #[test]
    fn crop_rect_full_and_degenerate() {
        let full = CropState { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };
        assert_eq!(full.pixel_rect(640, 480), (0, 0, 640, 480));
        // Swapped / zero-size selections still yield a valid 1×1 rect.
        let flat = CropState { x0: 1.0, y0: 1.0, x1: 1.0, y1: 1.0 };
        let (x, y, w, h) = flat.pixel_rect(10, 10);
        assert!(x + w <= 10 && y + h <= 10 && w >= 1 && h >= 1);
    }

    #[test]
    fn resize_specs() {
        assert_eq!(parse_resize("50%", 800, 600), Some((400, 300)));
        assert_eq!(parse_resize("400", 800, 600), Some((400, 300)));
        assert_eq!(parse_resize("x300", 800, 600), Some((400, 300)));
        assert_eq!(parse_resize("400x400", 800, 600), Some((400, 300)));
        assert_eq!(parse_resize("400X400!", 800, 600), Some((400, 400)));
        assert_eq!(parse_resize("100 × 50", 800, 600), Some((67, 50)));
        for bad in ["", "abc", "0", "-5%", "x", "10x", "99999999"] {
            assert_eq!(parse_resize(bad, 800, 600), None, "{bad:?}");
        }
    }

    #[test]
    fn save_image_handles_alpha_into_jpeg() {
        let dir = std::env::temp_dir().join(format!("ipeek-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rgba = DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4));
        for name in ["a.jpg", "a.png", "a.bmp", "a.webp"] {
            save_image(&rgba, &dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(image::open(dir.join(name)).is_ok(), "{name}");
        }
        assert!(save_image(&rgba, &dir.join("a.unknown")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn menu_navigation_skips_separators() {
        let first = first_op_index();
        assert!(MENU[first].label().is_some());
        for (i, item) in MENU.iter().enumerate() {
            if item.label().is_some() {
                assert!(MENU[next_op_index(i, 1)].label().is_some());
                assert!(MENU[next_op_index(i, -1)].label().is_some());
            }
        }
        // Wraps around from the last op to the first.
        let last = MENU.iter().rposition(|m| m.label().is_some()).unwrap();
        assert_eq!(next_op_index(last, 1), first);
    }
}
