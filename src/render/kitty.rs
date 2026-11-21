use std::io::{self, Cursor, Write};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::DynamicImage;

use super::Renderer;

const CHUNK_SIZE: usize = 4096;

pub struct KittyRenderer;

impl Renderer for KittyRenderer {
    fn render_fill(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        let b64 = STANDARD.encode(buf.into_inner());

        let (c, r) = (cols.max(1), rows.max(1));

        let stdout = io::stdout();
        let mut out = stdout.lock();

        // Kitty graphics protocol: APC sequences in chunks.
        // f=100 (PNG), a=T (transmit + display); giving both c and r makes
        // the terminal scale the image to exactly that cell box.
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
