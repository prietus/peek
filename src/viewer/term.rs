use std::io::{self, Write};

use anyhow::Result;
use crossterm::cursor;
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};

use super::CELL_ASPECT_H_OVER_W;

/// DEC private mode 2026 — synchronized update. Terminals that don't recognise
/// the escape ignore it silently, so it's safe to send unconditionally.
pub(super) fn begin_sync() -> Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b[?2026h")?;
    out.flush()?;
    Ok(())
}

pub(super) fn end_sync() -> Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b[?2026l")?;
    out.flush()?;
    Ok(())
}

/// Compute the cell rectangle the rendered image occupies inside (cols, rows).
/// Mirrors the aspect-preserving fit logic each protocol's renderer applies, so
/// overlays can be drawn relative to the image instead of the full screen.
pub(super) fn image_cell_rect(img_w: u32, img_h: u32, cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    let img_aspect = img_w as f64 / img_h.max(1) as f64;
    let target = img_aspect * CELL_ASPECT_H_OVER_W;
    let avail = cols as f64 / rows.max(1) as f64;
    let (c, r) = if target >= avail {
        let c = cols;
        let r = ((c as f64) / target).round() as u16;
        (c, r)
    } else {
        let r = rows;
        let c = ((r as f64) * target).round() as u16;
        (c, r)
    };
    let c = c.max(1).min(cols);
    let r = r.max(1).min(rows);
    // Renderers anchor the image at (0, 0); overlays should match.
    (0, 0, c, r)
}

pub(super) struct TerminalGuard;

impl TerminalGuard {
    pub(super) fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_rect_preserves_aspect() {
        // Square image in a wide terminal: limited by rows, 2 cols per row.
        assert_eq!(image_cell_rect(100, 100, 200, 50), (0, 0, 100, 50));
        // Very wide image: limited by columns.
        assert_eq!(image_cell_rect(1000, 100, 80, 40), (0, 0, 80, 4));
    }
}
