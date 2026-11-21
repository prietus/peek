use std::io::{self, Write};

use anyhow::Result;
use crossterm::cursor;
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};


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

/// Cell rectangle (x, y, w, h) the rendered image occupies inside
/// (cols, rows). Renderers anchor the image at (0, 0) and size it with
/// `render::fit_cells`, so overlays can be drawn relative to the image.
pub(super) fn image_cell_rect(img_w: u32, img_h: u32, cols: u16, rows: u16) -> (u16, u16, u16, u16) {
    let (c, r) = crate::render::fit_cells(img_w, img_h, cols, rows);
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

/// Hand the terminal back to a child process (normal screen, cooked mode)
/// and restore the viewer's state afterwards.
pub(super) fn with_terminal_released<T>(f: impl FnOnce() -> T) -> Result<T> {
    execute!(io::stdout(), cursor::Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    let result = f();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
    Ok(result)
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
