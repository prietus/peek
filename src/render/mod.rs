mod halfblocks;
mod iterm;
mod kitty;
mod sixel;

use anyhow::Result;
use image::DynamicImage;

use crate::detect::Protocol;

pub trait Renderer {
    fn render(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()>;
}

pub fn for_protocol(p: Protocol) -> Box<dyn Renderer> {
    match p {
        Protocol::Kitty => Box::new(kitty::KittyRenderer),
        Protocol::Iterm => Box::new(iterm::ItermRenderer),
        Protocol::Sixel => Box::new(sixel::SixelRenderer),
        Protocol::Halfblocks => Box::new(halfblocks::HalfblocksRenderer),
    }
}

/// Compute a target pixel size that fits the image into the terminal
/// while preserving aspect ratio. Assumes a cell is ~2:1 (h:w).
pub fn fit_to_cells(img_w: u32, img_h: u32, cols: u16, rows: u16) -> (u32, u32) {
    // Reserve one row so the prompt doesn't push the image up.
    let avail_rows = rows.saturating_sub(1).max(1) as u32;
    let avail_cols = cols.max(1) as u32;

    // A terminal cell is roughly twice as tall as wide, so pixels-per-cell:
    let cell_px_w = 1u32;
    let cell_px_h = 2u32;

    let max_px_w = avail_cols * cell_px_w;
    let max_px_h = avail_rows * cell_px_h;

    let scale = f64::min(
        max_px_w as f64 / img_w as f64,
        max_px_h as f64 / img_h as f64,
    )
    .min(1.0);

    let w = ((img_w as f64) * scale).round().max(1.0) as u32;
    let h = ((img_h as f64) * scale).round().max(1.0) as u32;
    (w, h)
}
