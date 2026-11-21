mod halfblocks;
mod iterm;
mod kitty;
mod sixel;

use anyhow::Result;
use image::DynamicImage;

use crate::detect::Protocol;

pub trait Renderer {
    /// Draw `img` at the cursor stretched to exactly `cols × rows` cells.
    fn render_fill(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()>;

    /// Draw `img` at the cursor as large as fits in `cols × rows` cells,
    /// keeping its aspect ratio.
    fn render(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        let (c, r) = fit_cells(img.width(), img.height(), cols, rows);
        self.render_fill(img, c, r)
    }
}

pub fn for_protocol(p: Protocol) -> Box<dyn Renderer> {
    match p {
        Protocol::Kitty => Box::new(kitty::KittyRenderer),
        Protocol::Iterm => Box::new(iterm::ItermRenderer),
        Protocol::Sixel => Box::new(sixel::SixelRenderer),
        Protocol::Halfblocks => Box::new(halfblocks::HalfblocksRenderer),
    }
}

/// Pick the largest (cols, rows) cell box that preserves the image aspect
/// ratio inside `max_cols × max_rows`, using the terminal's real cell shape.
/// Renderers and overlays all size images through this, so overlays (crop
/// frame) line up with what was drawn.
pub fn fit_cells(img_w: u32, img_h: u32, max_cols: u16, max_rows: u16) -> (u16, u16) {
    fit_cells_with_aspect(img_w, img_h, max_cols, max_rows, cell_aspect())
}

/// Height / width of a terminal cell (≈2 for most fonts).
pub fn cell_aspect() -> f64 {
    let (w, h) = cell_pixel_size();
    h as f64 / w as f64
}

fn fit_cells_with_aspect(
    img_w: u32,
    img_h: u32,
    max_cols: u16,
    max_rows: u16,
    cell_aspect: f64,
) -> (u16, u16) {
    let max_cols = max_cols.max(1);
    let max_rows = max_rows.max(1);
    let img_aspect = img_w as f64 / img_h.max(1) as f64;
    // Aspect ratio expressed in CELLS rather than pixels: a square image needs
    // more columns than rows because cells are taller than wide.
    let target_cell_aspect = img_aspect * cell_aspect;
    let avail_cell_aspect = max_cols as f64 / max_rows as f64;

    let (c, r) = if target_cell_aspect >= avail_cell_aspect {
        let c = max_cols;
        let r = ((c as f64) / target_cell_aspect).round() as u16;
        (c, r)
    } else {
        let r = max_rows;
        let c = ((r as f64) * target_cell_aspect).round() as u16;
        (c, r)
    };
    (c.clamp(1, max_cols), r.clamp(1, max_rows))
}

/// Size of one terminal cell in pixels, as reported by the terminal
/// (TIOCGWINSZ). Falls back to 10×20 when the terminal doesn't report it.
pub fn cell_pixel_size() -> (u32, u32) {
    match crossterm::terminal::window_size() {
        Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => (
            (ws.width / ws.columns).max(1) as u32,
            (ws.height / ws.rows).max(1) as u32,
        ),
        _ => (10, 20),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cells_preserves_aspect() {
        // Square image in a wide terminal: limited by rows, 2 cols per row.
        assert_eq!(fit_cells_with_aspect(100, 100, 200, 50, 2.0), (100, 50));
        // Taller cells need proportionally more columns.
        assert_eq!(fit_cells_with_aspect(100, 100, 200, 40, 2.5), (100, 40));
        // Very wide image: limited by columns.
        assert_eq!(fit_cells_with_aspect(1000, 100, 80, 40, 2.0), (80, 4));
        // Degenerate inputs never yield a zero-sized box.
        assert_eq!(fit_cells_with_aspect(1, 100_000, 80, 40, 2.0), (1, 40));
    }
}
