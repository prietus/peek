use std::io::{self, Write};

use anyhow::Result;
use image::{DynamicImage, imageops::FilterType};

use super::Renderer;

pub struct HalfblocksRenderer;

impl Renderer for HalfblocksRenderer {
    fn render_fill(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        // Each cell shows two vertical pixels (upper + lower half block).
        let cell_w = cols.max(1) as u32;
        let cell_h = rows.max(1) as u32 * 2;

        let resized = img
            .resize_exact(cell_w, cell_h, FilterType::Triangle)
            .to_rgba8();

        let stdout = io::stdout();
        let mut out = stdout.lock();
        let w = resized.width();
        let h = resized.height();

        for y in (0..h).step_by(2) {
            for x in 0..w {
                let top = resized.get_pixel(x, y).0;
                let bot = if y + 1 < h {
                    resized.get_pixel(x, y + 1).0
                } else {
                    [0, 0, 0, 0]
                };

                if top[3] == 0 && bot[3] == 0 {
                    write!(out, "\x1b[0m ")?;
                    continue;
                }

                // Foreground = top half, background = bottom half, glyph = upper half block.
                write!(
                    out,
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m\u{2580}",
                    top[0], top[1], top[2], bot[0], bot[1], bot[2]
                )?;
            }
            writeln!(out, "\x1b[0m")?;
        }
        Ok(())
    }
}
