use std::io::{self, Write};

use anyhow::{Result, anyhow};
use icy_sixel::SixelImage;
use image::{DynamicImage, imageops::FilterType};

use super::{Renderer, cell_pixel_size};

pub struct SixelRenderer;

impl Renderer for SixelRenderer {
    fn render_fill(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        // Sixel is pixel-addressed: convert the cell box to pixels with the
        // terminal's real cell size.
        let (cell_px_w, cell_px_h) = cell_pixel_size();
        let target_w = cols.max(1) as u32 * cell_px_w;
        let target_h = rows.max(1) as u32 * cell_px_h;

        let resized = img
            .resize_exact(target_w, target_h, FilterType::Triangle)
            .to_rgba8();

        let encoded = SixelImage::from_rgba(
            resized.as_raw().to_vec(),
            resized.width() as usize,
            resized.height() as usize,
        )
        .encode()
        .map_err(|e| anyhow!("sixel encode failed: {e:?}"))?;

        let stdout = io::stdout();
        let mut out = stdout.lock();
        out.write_all(encoded.as_bytes())?;
        writeln!(out)?;
        Ok(())
    }
}
