use std::io::{self, Write};

use anyhow::Result;
use crossterm::cursor;
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use image::DynamicImage;

use super::edit::{CropState, MENU, MenuItem};
use super::term::{begin_sync, end_sync, image_cell_rect};
use super::Viewer;
use crate::grid;
use crate::util::{human_bytes, pad_or_truncate, truncate_middle};

impl Viewer {
    /// Full repaint: clear, render image, status bar, optionally the menu.
    /// Used when image content or geometry changes (load, edit op, resize, navigate).
    pub(super) fn redraw_all(&mut self) -> Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));

        // Decode the new image BEFORE touching the screen — the old frame
        // stays visible during the (potentially slow) PNG decode, so the
        // user doesn't see a black gap mid-navigation.
        let load_error = if self.grid.is_none() {
            self.ensure_loaded().err().map(|e| e.to_string())
        } else {
            None
        };

        // Wrap the visible changes in a synchronized update so terminals that
        // support it (Kitty, iTerm2, WezTerm, foot, …) apply them atomically.
        // Terminals that don't support it ignore the escapes silently.
        begin_sync()?;

        {
            let mut out = io::stdout().lock();
            queue!(out, ResetColor, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
            out.flush()?;
        }

        if let Some(grid) = self.grid.as_mut() {
            grid::render(
                grid,
                &self.files,
                &self.marked,
                self.renderer.as_ref(),
                cols,
                rows,
            )?;
        } else if let Some(err) = load_error {
            let path = &self.files[self.index];
            let mut out = io::stdout().lock();
            queue!(out, Print(format!("failed to load {}: {err}", path.display())))?;
            out.flush()?;
        } else {
            let render_rows = rows.saturating_sub(1).max(1);
            let view = self.view_image();
            let img: &DynamicImage = match view.as_ref() {
                Some(cropped) => cropped,
                None => self.loaded.as_ref().unwrap().current(),
            };
            self.renderer.render(img, cols, render_rows)?;
        }

        self.draw_status_bar(cols, rows)?;

        if let Some(crop) = self.crop {
            self.draw_crop_overlay(cols, rows, &crop)?;
        }
        if let Some(sel) = self.menu_selected {
            self.draw_menu(cols, rows, sel)?;
        }
        if self.help_open {
            self.draw_help_popup(cols, rows)?;
        } else if self.info_open {
            self.draw_info_popup(cols, rows)?;
        }

        end_sync()?;
        Ok(())
    }

    /// Cheap repaint: only writes the menu cells. Does NOT clear the screen or
    /// re-transmit the image, so no flicker when scrolling through menu items.
    pub(super) fn redraw_menu_only(&self) -> Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        if let Some(sel) = self.menu_selected {
            self.draw_menu(cols, rows, sel)?;
        }
        Ok(())
    }

    /// Cheap repaint of only the status row.
    pub(super) fn draw_status_only(&self) -> Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        self.draw_status_bar(cols, rows)
    }

    pub(super) fn draw_status_bar(&self, cols: u16, rows: u16) -> Result<()> {
        let (bg, fg, line) = self.status_line(cols);

        let mut out = io::stdout().lock();
        queue!(
            out,
            cursor::MoveTo(0, rows.saturating_sub(1)),
            SetBackgroundColor(bg),
            SetForegroundColor(fg),
            Print(line),
            ResetColor,
        )?;
        out.flush()?;
        Ok(())
    }

    pub(super) fn status_line(&self, cols: u16) -> (Color, Color, String) {
        let width = cols as usize;

        // Save-as input prompt takes priority over everything else.
        if let Some(prompt) = self.prompt.as_ref() {
            let (bg, fg, content) = self.prompt_line(prompt);
            return (bg, fg, pad_or_truncate(&content, width));
        }

        if let Some(crop) = self.crop.as_ref() {
            let bg = Color::Rgb { r: 90, g: 60, b: 20 };
            let fg = Color::Rgb { r: 255, g: 235, b: 200 };
            let (w, h) = self
                .loaded
                .as_ref()
                .map(|l| (l.img.width(), l.img.height()))
                .unwrap_or((0, 0));
            let cw = ((crop.x1 - crop.x0).abs() * w as f32).round() as u32;
            let ch = ((crop.y1 - crop.y0).abs() * h as f32).round() as u32;
            let content = format!(
                " crop {cw}×{ch} px · hjkl move · HJKL resize · a all · Enter apply · Esc cancel"
            );
            return (bg, fg, pad_or_truncate(&content, width));
        }

        if self.confirm_delete {
            let bg = Color::Rgb { r: 130, g: 30, b: 30 };
            let fg = Color::Rgb { r: 255, g: 230, b: 230 };
            let target = self
                .grid
                .as_ref()
                .map(|g| g.selected)
                .unwrap_or(self.index);
            let name = self
                .files
                .get(target)
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let content = format!(" Delete {name}? (y/N)");
            return (bg, fg, pad_or_truncate(&content, width));
        }

        let bg = Color::DarkGrey;
        let fg = Color::White;

        if let Some(msg) = self.status_msg.as_ref() {
            let content = format!(" {msg}");
            return (bg, fg, pad_or_truncate(&content, width));
        }

        if let Some(grid) = self.grid.as_ref() {
            let sel_path = &self.files[grid.selected];
            let name = sel_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let marks = if self.marked.is_empty() {
                String::new()
            } else {
                format!("  ✓{}", self.marked.len())
            };
            let left = format!(
                " grid · {}/{}{}{marks}  {}",
                grid.selected + 1,
                self.files.len(),
                self.filter_badge(),
                name
            );
            let right = " Space mark · / filter · ? help · q quit ";
            let pad = width.saturating_sub(left.chars().count() + right.chars().count());
            let line = format!("{left}{}{right}", " ".repeat(pad));
            return (bg, fg, line.chars().take(width).collect());
        }

        let path = &self.files[self.index];
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let modified = self.loaded.as_ref().map(|l| l.modified).unwrap_or(false);
        let flag = if modified { " [+]" } else { "" };
        let zoom = if self.zoom > 1.001 {
            format!("  {:.2}x", self.zoom)
        } else {
            String::new()
        };

        let play = if self.slideshow_running { "▶ " } else { "" };
        let frame_info = self
            .loaded
            .as_ref()
            .and_then(|l| l.frames.as_ref().map(|f| (l.frame_idx + 1, f.len())))
            .map(|(i, n)| format!("  [{i}/{n}f]"))
            .unwrap_or_default();
        let left = format!(
            " {play}{}/{}{}  {}{}{}{}",
            self.index + 1,
            self.files.len(),
            self.filter_badge(),
            name,
            flag,
            zoom,
            frame_info,
        );
        let right = " +/- zoom · / filter · t grid · ? help · q quit ";
        let pad = width.saturating_sub(left.chars().count() + right.chars().count());
        let line = format!("{left}{}{right}", " ".repeat(pad));
        (bg, fg, line.chars().take(width).collect())
    }

    /// " [/query]" when a filter is narrowing the listing, else "".
    fn filter_badge(&self) -> String {
        self.filter
            .as_ref()
            .map(|f| format!(" [/{f}]"))
            .unwrap_or_default()
    }

    pub(super) fn draw_menu(&self, cols: u16, rows: u16, selected: usize) -> Result<()> {
        // Explicit RGB so the menu looks the same regardless of terminal theme.
        let bg = Color::Rgb { r: 20, g: 20, b: 28 };
        let fg = Color::Rgb { r: 220, g: 220, b: 220 };
        let sel_bg = Color::Rgb { r: 70, g: 130, b: 200 };
        let sel_fg = Color::Rgb { r: 255, g: 255, b: 255 };

        let inner_width = MENU
            .iter()
            .filter_map(|item| match item {
                MenuItem::Op(label, _) => Some(label.chars().count()),
                MenuItem::Separator => None,
            })
            .max()
            .unwrap_or(20)
            + 2;
        let box_width = inner_width + 2;
        let box_height = MENU.len() + 2;

        let start_col = cols.saturating_sub(box_width as u16) / 2;
        let start_row = rows.saturating_sub(box_height as u16) / 2;

        let mut out = io::stdout().lock();

        queue!(
            out,
            cursor::MoveTo(start_col, start_row),
            SetBackgroundColor(bg),
            SetForegroundColor(fg),
            Print(format!("╭{}╮", "─".repeat(inner_width))),
        )?;

        for (i, item) in MENU.iter().enumerate() {
            queue!(out, cursor::MoveTo(start_col, start_row + 1 + i as u16))?;
            match item {
                MenuItem::Op(label, _) => {
                    let is_selected = i == selected;
                    let content = format!(" {label:<width$} ", width = inner_width - 2);
                    queue!(out, SetBackgroundColor(bg), SetForegroundColor(fg), Print("│"))?;
                    if is_selected {
                        queue!(
                            out,
                            SetBackgroundColor(sel_bg),
                            SetForegroundColor(sel_fg),
                            Print(&content),
                            SetBackgroundColor(bg),
                            SetForegroundColor(fg),
                        )?;
                    } else {
                        queue!(out, Print(&content))?;
                    }
                    queue!(out, Print("│"))?;
                }
                MenuItem::Separator => {
                    queue!(out, Print(format!("├{}┤", "─".repeat(inner_width))))?;
                }
            }
        }

        queue!(
            out,
            cursor::MoveTo(start_col, start_row + 1 + MENU.len() as u16),
            Print(format!("╰{}╯", "─".repeat(inner_width))),
            ResetColor,
        )?;
        out.flush()?;
        Ok(())
    }

    pub(super) fn draw_crop_overlay(&self, cols: u16, rows: u16, crop: &CropState) -> Result<()> {
        let Some(loaded) = self.loaded.as_ref() else {
            return Ok(());
        };
        let img = loaded.current();
        let render_rows = rows.saturating_sub(1).max(1);
        let (img_x, img_y, img_w, img_h) =
            image_cell_rect(img.width(), img.height(), cols, render_rows);
        if img_w < 2 || img_h < 2 {
            return Ok(());
        }

        let lo_x = crop.x0.min(crop.x1).clamp(0.0, 1.0);
        let hi_x = crop.x0.max(crop.x1).clamp(0.0, 1.0);
        let lo_y = crop.y0.min(crop.y1).clamp(0.0, 1.0);
        let hi_y = crop.y0.max(crop.y1).clamp(0.0, 1.0);
        let span_w = img_w as f32 - 1.0;
        let span_h = img_h as f32 - 1.0;
        let cx0 = img_x + (lo_x * span_w).round() as u16;
        let cy0 = img_y + (lo_y * span_h).round() as u16;
        let cx1 = img_x + (hi_x * span_w).round() as u16;
        let cy1 = img_y + (hi_y * span_h).round() as u16;
        if cx1 <= cx0 || cy1 <= cy0 {
            return Ok(());
        }

        let color = Color::Rgb { r: 255, g: 220, b: 80 };
        let mut out = io::stdout().lock();
        queue!(out, SetForegroundColor(color))?;

        // Corners
        queue!(out, cursor::MoveTo(cx0, cy0), Print("┏"))?;
        queue!(out, cursor::MoveTo(cx1, cy0), Print("┓"))?;
        queue!(out, cursor::MoveTo(cx0, cy1), Print("┗"))?;
        queue!(out, cursor::MoveTo(cx1, cy1), Print("┛"))?;

        // Top + bottom edges
        if cx1 > cx0 + 1 {
            let span = (cx1 - cx0 - 1) as usize;
            queue!(out, cursor::MoveTo(cx0 + 1, cy0), Print("━".repeat(span)))?;
            queue!(out, cursor::MoveTo(cx0 + 1, cy1), Print("━".repeat(span)))?;
        }

        // Side edges
        for y in (cy0 + 1)..cy1 {
            queue!(out, cursor::MoveTo(cx0, y), Print("┃"))?;
            queue!(out, cursor::MoveTo(cx1, y), Print("┃"))?;
        }

        queue!(out, ResetColor)?;
        out.flush()?;
        Ok(())
    }

    pub(super) fn draw_help_popup(&self, cols: u16, rows: u16) -> Result<()> {
        let lines: Vec<(String, String)> = vec![
            ("n / Space / →".into(), "next image".into()),
            ("p / ←".into(), "previous image".into()),
            ("j k h l".into(), "navigate (pan when zoomed)".into()),
            ("g G".into(), "first / last".into()),
            ("t Tab".into(), "toggle grid".into()),
            ("/".into(), "filter by name (empty = clear)".into()),
            (":".into(), "go to image number".into()),
            ("O".into(), "cycle sort: name/date/size/random".into()),
            (String::new(), String::new()),
            ("Space".into(), "(grid) mark / unmark".into()),
            ("A · u".into(), "(grid) mark all · clear".into()),
            ("y · M".into(), "(grid) copy · move marked to dir".into()),
            (String::new(), String::new()),
            ("+ =".into(), "zoom in".into()),
            ("-".into(), "zoom out".into()),
            ("0".into(), "reset zoom & pan".into()),
            ("i".into(), "info panel (also in grid)".into()),
            (String::new(), String::new()),
            ("r R".into(), "rotate CW / CCW".into()),
            ("f F".into(), "flip horizontal / vertical".into()),
            ("m".into(), "edit menu".into()),
            ("c".into(), "crop (hjkl move · HJKL resize)".into()),
            ("s S".into(), "save / save-as".into()),
            (String::new(), String::new()),
            ("d".into(), "delete file (with y/N prompt)".into()),
            ("Ctrl+R".into(), "reload from disk".into()),
            ("P".into(), "slideshow play / pause".into()),
            (String::new(), String::new()),
            ("?".into(), "this help".into()),
            ("q / Esc".into(), "close popup · quit".into()),
        ];
        self.draw_text_box("ipeek — keymap", &lines, cols, rows)
    }

    pub(super) fn draw_info_popup(&self, cols: u16, rows: u16) -> Result<()> {
        // In grid mode the popup describes the highlighted thumb; in view
        // mode it describes the image currently on screen.
        let target_index = self
            .grid
            .as_ref()
            .map(|g| g.selected)
            .unwrap_or(self.index);
        let path = &self.files[target_index];
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let full = path.display().to_string();
        let fmt = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_uppercase())
            .unwrap_or_else(|| "?".into());
        let loaded = self.loaded.as_ref().filter(|l| &l.path == path);
        // Cheap header-only dimension lookup when the image isn't in memory.
        let dims = loaded
            .map(|l| format!("{} × {}", l.img.width(), l.img.height()))
            .or_else(|| {
                image::ImageReader::open(path)
                    .ok()?
                    .with_guessed_format()
                    .ok()?
                    .into_dimensions()
                    .ok()
                    .map(|(w, h)| format!("{w} × {h}"))
            })
            .unwrap_or_else(|| "(unavailable)".into());
        let modified = loaded.map(|l| l.modified).unwrap_or(false);
        let frames = loaded.and_then(|l| l.frames.as_ref().map(|f| f.len()));
        let disk = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        let max_path = (cols as usize).saturating_sub(20).max(20);
        let mut lines: Vec<(String, String)> = vec![
            ("name".into(), name),
            ("path".into(), truncate_middle(&full, max_path)),
            ("format".into(), fmt),
            ("size".into(), dims),
            ("disk".into(), human_bytes(disk)),
            (
                "modified".into(),
                if modified { "yes (unsaved)".into() } else { "no".into() },
            ),
        ];
        if let Some(n) = frames {
            lines.push(("frames".into(), format!("{n} (animated)")));
        }
        self.draw_text_box("image info", &lines, cols, rows)
    }

    pub(super) fn draw_text_box(
        &self,
        title: &str,
        lines: &[(String, String)],
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let bg = Color::Rgb { r: 20, g: 20, b: 28 };
        let fg = Color::Rgb { r: 220, g: 220, b: 220 };
        let dim = Color::Rgb { r: 150, g: 150, b: 170 };

        let left_w = lines.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0);
        let right_w = lines.iter().map(|(_, r)| r.chars().count()).max().unwrap_or(0);
        let gap = 3;
        let content_w = (left_w + gap + right_w).max(title.chars().count() + 4);
        let inner = content_w + 2; // 1 char padding each side

        let box_w = inner + 2;
        let box_h = lines.len() + 2;
        if (box_w as u16) > cols || (box_h as u16) > rows.saturating_sub(1) {
            return Ok(());
        }
        let start_col = cols.saturating_sub(box_w as u16) / 2;
        let start_row = rows.saturating_sub(box_h as u16) / 2;

        let mut out = io::stdout().lock();

        let title_str = format!(" {title} ");
        let title_n = title_str.chars().count();
        let lead = 2;
        let trail = inner.saturating_sub(lead + title_n);
        queue!(
            out,
            cursor::MoveTo(start_col, start_row),
            SetBackgroundColor(bg),
            SetForegroundColor(fg),
            Print(format!(
                "╭{}{}{}╮",
                "─".repeat(lead),
                title_str,
                "─".repeat(trail),
            )),
        )?;

        for (i, (l, r)) in lines.iter().enumerate() {
            queue!(out, cursor::MoveTo(start_col, start_row + 1 + i as u16))?;
            if l.is_empty() && r.is_empty() {
                queue!(
                    out,
                    SetBackgroundColor(bg),
                    SetForegroundColor(fg),
                    Print(format!("│{}│", " ".repeat(inner))),
                )?;
            } else {
                queue!(
                    out,
                    SetBackgroundColor(bg),
                    SetForegroundColor(fg),
                    Print("│ "),
                    SetForegroundColor(dim),
                    Print(format!("{l:<left_w$}")),
                    Print(" ".repeat(gap)),
                    SetForegroundColor(fg),
                    Print(format!("{r:<right_w$}")),
                    Print(" │"),
                )?;
            }
        }

        queue!(
            out,
            cursor::MoveTo(start_col, start_row + 1 + lines.len() as u16),
            SetBackgroundColor(bg),
            SetForegroundColor(fg),
            Print(format!("╰{}╯", "─".repeat(inner))),
            ResetColor,
        )?;
        out.flush()?;
        Ok(())
    }
}
