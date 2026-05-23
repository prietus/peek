use std::io::{self, Write};

use anyhow::{Result, anyhow};
use icy_sixel::SixelImage;
use image::{DynamicImage, imageops::FilterType};

use super::{Renderer, fit_to_cells};

pub struct SixelRenderer;

impl Renderer for SixelRenderer {
    fn render(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        // Sixel needs pixel dimensions; estimate from cell grid (cell ~ 1×2 px ratio
        // is too coarse for a graphic protocol, so use a richer 10×20 multiplier).
        let cell_px_w = 10u32;
        let cell_px_h = 20u32;
        let (cell_w, cell_h) = fit_to_cells(img.width(), img.height(), cols, rows);
        let target_w = cell_w * cell_px_w;
        let target_h = cell_h * cell_px_h;

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
