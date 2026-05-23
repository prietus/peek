use std::io::{self, Cursor, Write};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::DynamicImage;

use super::Renderer;

pub struct ItermRenderer;

impl Renderer for ItermRenderer {
    fn render(&self, img: &DynamicImage, cols: u16, rows: u16) -> Result<()> {
        // Encode the image as PNG and pass it inline. iTerm2 will scale it
        // using width/height args expressed in character cells.
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        let png = buf.into_inner();
        let b64 = STANDARD.encode(&png);

        // Reserve one row for the prompt.
        let max_rows = rows.saturating_sub(1).max(1);

        let stdout = io::stdout();
        let mut out = stdout.lock();
        // OSC 1337 ; File = [args] : base64 BEL
        write!(
            out,
            "\x1b]1337;File=inline=1;preserveAspectRatio=1;width={cols};height={max_rows}:{b64}\x07"
        )?;
        writeln!(out)?;
        Ok(())
    }
}
