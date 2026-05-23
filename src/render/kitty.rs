use std::io::{self, Cursor, Write};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::DynamicImage;

use super::Renderer;

const CHUNK_SIZE: usize = 4096;
/// Terminal cells are ~twice as tall as wide. Used to translate image aspect
/// (in pixels) to cell aspect for sizing the Kitty placement.
const CELL_ASPECT_H_OVER_W: f64 = 2.0;

pub struct KittyRenderer;

impl Renderer for KittyRenderer {
    fn render(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        let b64 = STANDARD.encode(buf.into_inner());

        let max_rows = rows.max(1);
        let (c, r) = fit_cells(img.width(), img.height(), cols.max(1), max_rows);

        let stdout = io::stdout();
        let mut out = stdout.lock();

        // Kitty graphics protocol: APC sequences in chunks.
        // f=100 (PNG), a=T (transmit + display), c/r are computed to preserve
        // aspect ratio (specifying both forces scaling).
        let total = b64.len();
        let mut start = 0;
        let mut first = true;
        while start < total {
            let end = (start + CHUNK_SIZE).min(total);
            let more = if end < total { 1 } else { 0 };
            let chunk = &b64[start..end];

            if first {
                // z=-1 places the image below the text layer so overlays
                // (status bar, popup menu) draw on top of it.
                write!(
                    out,
                    "\x1b_Gf=100,a=T,c={c},r={r},z=-1,m={more};{chunk}\x1b\\"
                )?;
                first = false;
            } else {
                write!(out, "\x1b_Gm={more};{chunk}\x1b\\")?;
            }
            start = end;
        }
        writeln!(out)?;
        Ok(())
    }
}

/// Pick the largest (cols, rows) cell box that preserves the image aspect
/// ratio inside the available `max_cols × max_rows`.
fn fit_cells(img_w: u32, img_h: u32, max_cols: u16, max_rows: u16) -> (u16, u16) {
    let img_aspect = img_w as f64 / img_h.max(1) as f64;
    // Aspect ratio expressed in CELLS rather than pixels: a square image needs
    // more columns than rows because cells are taller than wide.
    let target_cell_aspect = img_aspect * CELL_ASPECT_H_OVER_W;
    let avail_cell_aspect = max_cols as f64 / max_rows.max(1) as f64;

    let (c, r) = if target_cell_aspect >= avail_cell_aspect {
        let c = max_cols;
        let r = ((c as f64) / target_cell_aspect).round() as u16;
        (c, r)
    } else {
        let r = max_rows;
        let c = ((r as f64) * target_cell_aspect).round() as u16;
        (c, r)
    };
    (c.max(1).min(max_cols), r.max(1).min(max_rows))
}
