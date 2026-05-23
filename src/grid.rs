use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::cursor;
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use image::{DynamicImage, GenericImage, Rgba, RgbaImage, imageops::FilterType};

use crate::render::Renderer;

/// Max pixel side for cached thumbnails. Bigger = sharper but more memory.
const THUMB_MAX: u32 = 256;
/// Each thumbnail slot occupies this many cells in the grid (last row reserved for label).
const THUMB_CELLS_W: u16 = 22;
const THUMB_CELLS_H: u16 = 11;
/// Canvas pixels per cell. Higher = sharper output but bigger canvas.
const PX_PER_CELL_W: u32 = 8;
const PX_PER_CELL_H: u32 = 16;

pub struct GridState {
    pub selected: usize,
    scroll_row: usize,
    cache: HashMap<PathBuf, DynamicImage>,
}

impl GridState {
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            scroll_row: 0,
            cache: HashMap::new(),
        }
    }

    pub fn move_by(&mut self, delta: i32, files_len: usize) {
        let new = (self.selected as i32 + delta).clamp(0, files_len as i32 - 1) as usize;
        self.selected = new;
    }

    pub fn jump_to(&mut self, target: usize, files_len: usize) {
        self.selected = target.min(files_len.saturating_sub(1));
    }

    /// Drop a path from the thumbnail cache — used after the file is deleted.
    pub fn forget(&mut self, path: &Path) {
        self.cache.remove(path);
    }

    /// Adjust scroll so the selected thumbnail is visible inside `grid_rows`.
    fn ensure_visible(&mut self, layout: &Layout) {
        let row = self.selected / layout.grid_cols as usize;
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + layout.grid_rows as usize {
            self.scroll_row = row + 1 - layout.grid_rows as usize;
        }
    }

    fn ensure_thumb(&mut self, path: &Path) -> Result<&DynamicImage> {
        if !self.cache.contains_key(path) {
            let thumb = crate::cache::load_or_build_thumb(path, THUMB_MAX)?;
            self.cache.insert(path.to_path_buf(), thumb);
        }
        Ok(self.cache.get(path).unwrap())
    }
}

struct Layout {
    grid_cols: u16,
    grid_rows: u16,
    canvas_w: u32,
    canvas_h: u32,
}

fn compute_layout(cols: u16, rows: u16) -> Layout {
    let avail_rows = rows.saturating_sub(1).max(THUMB_CELLS_H);
    let grid_cols = (cols / THUMB_CELLS_W).max(1);
    let grid_rows = (avail_rows / THUMB_CELLS_H).max(1);
    let canvas_w = grid_cols as u32 * THUMB_CELLS_W as u32 * PX_PER_CELL_W;
    let canvas_h = grid_rows as u32 * THUMB_CELLS_H as u32 * PX_PER_CELL_H;
    Layout {
        grid_cols,
        grid_rows,
        canvas_w,
        canvas_h,
    }
}

pub fn render(
    state: &mut GridState,
    files: &[PathBuf],
    marked: &HashSet<PathBuf>,
    renderer: &dyn Renderer,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let layout = compute_layout(cols, rows);
    state.ensure_visible(&layout);

    let canvas = build_canvas(state, files, &layout)?;
    // The renderer consumes the canvas as a single image and fits it to the
    // (cols × avail_rows) cell grid. The canvas aspect matches the grid so
    // each thumb lines up with its cell rectangle.
    let avail_rows = layout.grid_rows * THUMB_CELLS_H;
    renderer.render(&canvas, cols, avail_rows)?;

    draw_labels(state, files, marked, &layout)?;
    Ok(())
}

fn build_canvas(
    state: &mut GridState,
    files: &[PathBuf],
    layout: &Layout,
) -> Result<DynamicImage> {
    let bg = Rgba([20u8, 20, 28, 255]);
    let mut canvas = RgbaImage::from_pixel(layout.canvas_w, layout.canvas_h, bg);

    let slot_w_px = THUMB_CELLS_W as u32 * PX_PER_CELL_W;
    let slot_h_px = THUMB_CELLS_H as u32 * PX_PER_CELL_H;
    // Reserve the last cell row of the slot for the label.
    let thumb_max_px_w = slot_w_px - 4; // 2 px padding each side
    let thumb_max_px_h = (THUMB_CELLS_H - 1) as u32 * PX_PER_CELL_H - 4;

    let start_index = state.scroll_row * layout.grid_cols as usize;
    let visible = (layout.grid_cols * layout.grid_rows) as usize;
    let end_index = (start_index + visible).min(files.len());

    for i in start_index..end_index {
        let local = i - start_index;
        let col = local % layout.grid_cols as usize;
        let row = local / layout.grid_cols as usize;
        let slot_x = col as u32 * slot_w_px;
        let slot_y = row as u32 * slot_h_px;

        let thumb = match state.ensure_thumb(&files[i]) {
            Ok(t) => t,
            Err(_) => continue, // skip undecodable files; label still drawn
        };

        let resized = thumb
            .resize(thumb_max_px_w, thumb_max_px_h, FilterType::Triangle)
            .to_rgba8();

        // Center the thumb within its image area.
        let img_area_h = (THUMB_CELLS_H - 1) as u32 * PX_PER_CELL_H;
        let off_x = slot_x + (slot_w_px - resized.width()) / 2;
        let off_y = slot_y + (img_area_h - resized.height()) / 2;
        let _ = canvas.copy_from(&resized, off_x, off_y);
    }

    Ok(DynamicImage::ImageRgba8(canvas))
}

fn draw_labels(
    state: &GridState,
    files: &[PathBuf],
    marked: &HashSet<PathBuf>,
    layout: &Layout,
) -> Result<()> {
    let mut out = io::stdout().lock();

    let start_index = state.scroll_row * layout.grid_cols as usize;
    let visible = (layout.grid_cols * layout.grid_rows) as usize;
    let end_index = (start_index + visible).min(files.len());

    let label_fg = Color::Rgb { r: 220, g: 220, b: 220 };
    let label_bg = Color::Rgb { r: 20, g: 20, b: 28 };
    let sel_fg = Color::Rgb { r: 0, g: 0, b: 0 };
    let sel_bg = Color::Rgb { r: 100, g: 180, b: 255 };
    let border = Color::Rgb { r: 100, g: 180, b: 255 };
    let mark_fg = Color::Rgb { r: 120, g: 220, b: 130 };

    for i in start_index..end_index {
        let local = i - start_index;
        let col = (local % layout.grid_cols as usize) as u16;
        let row = (local / layout.grid_cols as usize) as u16;

        let cell_x = col * THUMB_CELLS_W;
        let cell_y = row * THUMB_CELLS_H;
        let label_y = cell_y + THUMB_CELLS_H - 1;

        let name = files[i]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let inner_w = (THUMB_CELLS_W - 2) as usize;
        let is_marked = marked.contains(&files[i]);
        let trimmed: String = name.chars().take(inner_w).collect();
        let rest = format!("{trimmed:<inner_w$} ", inner_w = inner_w);

        let is_selected = i == state.selected;
        if is_selected {
            // Top + bottom border across the slot.
            queue!(
                out,
                SetForegroundColor(border),
                SetBackgroundColor(label_bg),
                cursor::MoveTo(cell_x, cell_y),
                Print("─".repeat(THUMB_CELLS_W as usize)),
                cursor::MoveTo(cell_x, cell_y + THUMB_CELLS_H - 2),
                Print("─".repeat(THUMB_CELLS_W as usize)),
            )?;
            // Side borders for the image area only.
            for y in (cell_y + 1)..(cell_y + THUMB_CELLS_H - 2) {
                queue!(
                    out,
                    cursor::MoveTo(cell_x, y),
                    Print("│"),
                    cursor::MoveTo(cell_x + THUMB_CELLS_W - 1, y),
                    Print("│"),
                )?;
            }
        }

        // Label row — split so the ✓ stays green even on the selected (blue) row.
        let (bg, fg) = if is_selected {
            (sel_bg, sel_fg)
        } else {
            (label_bg, label_fg)
        };
        queue!(
            out,
            cursor::MoveTo(cell_x, label_y),
            SetBackgroundColor(bg),
        )?;
        if is_marked {
            queue!(out, SetForegroundColor(mark_fg), Print("✓"))?;
        } else {
            queue!(out, SetForegroundColor(fg), Print(" "))?;
        }
        queue!(out, SetForegroundColor(fg), Print(&rest))?;
    }

    queue!(out, ResetColor)?;
    out.flush()?;
    Ok(())
}
