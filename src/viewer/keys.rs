
use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::terminal;

use super::edit::{BRIGHTNESS_STEP, CONTRAST_STEP, CropState, EditOp, MENU, MenuItem, next_op_index, first_op_index};
use super::prompt::PromptKind;
use super::{CROP_STEP, ExportOp, Viewer};
use crate::grid::GridState;

impl Viewer {
    /// Help and Info popups are passive: any key closes them. q still quits.
    pub(super) fn handle_overlay_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        if matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        self.help_open = false;
        self.info_open = false;
        self.redraw_all()?;
        Ok(matches!(code, KeyCode::Char('q')))
    }

    pub(super) fn handle_confirm_delete_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        if matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.confirm_delete = false;
                self.delete_current()?;
            }
            _ => {
                self.confirm_delete = false;
                self.status_msg = Some("delete cancelled".into());
                self.draw_status_only()?;
            }
        }
        Ok(false)
    }

    pub(super) fn handle_crop_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        if matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        // Treat uppercase letters (HJKL) the same as Shift+arrow — terminals
        // usually emit the literal uppercase char without a SHIFT modifier.
        let shift = mods.contains(KeyModifiers::SHIFT)
            || matches!(code, KeyCode::Char(c) if c.is_ascii_uppercase());

        let Some(crop) = self.crop.as_mut() else {
            return Ok(false);
        };
        let step = CROP_STEP;
        let mut dirty = true;

        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.crop = None;
                self.status_msg = Some("crop cancelled".into());
            }
            KeyCode::Enter => {
                self.apply_crop()?;
                return Ok(false);
            }
            KeyCode::Char('a') => {
                crop.x0 = 0.0;
                crop.y0 = 0.0;
                crop.x1 = 1.0;
                crop.y1 = 1.0;
            }
            KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => {
                if shift {
                    crop.x1 = (crop.x1 - step).max(crop.x0 + step);
                } else {
                    let s = step.min(crop.x0);
                    crop.x0 -= s;
                    crop.x1 -= s;
                }
            }
            KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => {
                if shift {
                    crop.x1 = (crop.x1 + step).min(1.0);
                } else {
                    let s = step.min(1.0 - crop.x1);
                    crop.x0 += s;
                    crop.x1 += s;
                }
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up => {
                if shift {
                    crop.y1 = (crop.y1 - step).max(crop.y0 + step);
                } else {
                    let s = step.min(crop.y0);
                    crop.y0 -= s;
                    crop.y1 -= s;
                }
            }
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down => {
                if shift {
                    crop.y1 = (crop.y1 + step).min(1.0);
                } else {
                    let s = step.min(1.0 - crop.y1);
                    crop.y0 += s;
                    crop.y1 += s;
                }
            }
            _ => dirty = false,
        }
        if dirty {
            self.redraw_all()?;
        }
        Ok(false)
    }

    /// Returns true when the viewer should exit.
    pub(super) fn handle_view_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        // Any input clears the transient status message from the previous action.
        self.status_msg = None;
        let zoomed = self.zoom > 1.0;

        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => return Ok(true),

            // File navigation. When zoomed, hjkl/arrows pan instead — use n/p/Space.
            KeyCode::Char('n') | KeyCode::Char(' ') => self.navigate(1),
            KeyCode::Char('p') => self.navigate(-1),

            KeyCode::Char('j') | KeyCode::Down => {
                if zoomed {
                    self.pan(0.0, 1.0)?;
                } else {
                    self.navigate(1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if zoomed {
                    self.pan(0.0, -1.0)?;
                } else {
                    self.navigate(-1);
                }
            }
            KeyCode::Right => {
                if zoomed {
                    self.pan(1.0, 0.0)?;
                } else {
                    self.navigate(1);
                }
            }
            KeyCode::Left => {
                if zoomed {
                    self.pan(-1.0, 0.0)?;
                } else {
                    self.navigate(-1);
                }
            }
            KeyCode::Char('h') => self.pan(-1.0, 0.0)?,
            KeyCode::Char('l') => self.pan(1.0, 0.0)?,

            KeyCode::Char('g') | KeyCode::Home => self.jump_to(0),
            KeyCode::Char('G') | KeyCode::End => self.jump_to(self.files.len() - 1),

            // Zoom
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_in()?,
            KeyCode::Char('-') => self.zoom_out()?,
            KeyCode::Char('0') => self.reset_view()?,

            KeyCode::Char('r') if mods.contains(KeyModifiers::CONTROL) => self.reload_current()?,
            KeyCode::Char('r') => self.apply(EditOp::RotateCw)?,
            KeyCode::Char('R') => self.apply(EditOp::RotateCcw)?,
            KeyCode::Char('f') => self.apply(EditOp::FlipH)?,
            KeyCode::Char('F') => self.apply(EditOp::FlipV)?,
            KeyCode::Char('b') => self.apply(EditOp::Brightness(BRIGHTNESS_STEP))?,
            KeyCode::Char('B') => self.apply(EditOp::Brightness(-BRIGHTNESS_STEP))?,
            KeyCode::Char('e') => self.apply(EditOp::Contrast(CONTRAST_STEP))?,
            KeyCode::Char('E') => self.apply(EditOp::Contrast(-CONTRAST_STEP))?,
            KeyCode::Char('u') => self.undo()?,
            KeyCode::Char('z') => self.open_resize_prompt()?,

            KeyCode::Char('s') => self.save_current()?,
            KeyCode::Char('S') => self.open_prompt(PromptKind::SaveAs, String::new())?,

            KeyCode::Char('m') => {
                self.menu_selected = Some(first_op_index());
                self.redraw_menu_only()?;
            }
            KeyCode::Char('c') => {
                // Crop is relative to the full image, so drop any zoom/pan
                // first so what the user sees lines up with what gets cropped.
                let _ = self.ensure_loaded();
                if self.loaded.is_some() {
                    self.reset_view_state();
                    self.crop = Some(CropState::new());
                    self.redraw_all()?;
                }
            }
            KeyCode::Char('t') | KeyCode::Tab => {
                self.grid = Some(GridState::new(self.index));
                self.redraw_all()?;
            }
            KeyCode::Char('?') => {
                self.help_open = true;
                self.redraw_all()?;
            }
            KeyCode::Char('i') => {
                // Make sure dimensions are available for the info popup.
                let _ = self.ensure_loaded();
                self.info_open = true;
                self.redraw_all()?;
            }
            KeyCode::Char('d') => {
                self.confirm_delete = true;
                self.draw_status_only()?;
            }
            KeyCode::Char('P') => self.toggle_slideshow()?,
            _ => self.handle_common_key(code)?,
        }
        Ok(false)
    }

    pub(super) fn handle_grid_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        self.status_msg = None;
        let Some(grid) = self.grid.as_mut() else {
            return Ok(false);
        };
        // Compute current grid columns to translate j/k into row jumps.
        let (cols, _) = terminal::size().unwrap_or((80, 24));
        let grid_cols = (cols / 22).max(1) as i32;

        match code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => return Ok(true),

            KeyCode::Esc | KeyCode::Char('t') | KeyCode::Tab => {
                self.grid = None;
                self.redraw_all()?;
            }
            KeyCode::Enter => {
                let target = grid.selected;
                self.grid = None;
                if target == self.index {
                    // jump_to skips the repaint when the index is unchanged,
                    // which would leave the grid on screen.
                    self.redraw_all()?;
                } else {
                    self.jump_to(target);
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                grid.move_by(-1, self.files.len());
                self.redraw_all()?;
            }
            KeyCode::Char('l') | KeyCode::Right => {
                grid.move_by(1, self.files.len());
                self.redraw_all()?;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                grid.move_by(-grid_cols, self.files.len());
                self.redraw_all()?;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                grid.move_by(grid_cols, self.files.len());
                self.redraw_all()?;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                grid.jump_to(0, self.files.len());
                self.redraw_all()?;
            }
            KeyCode::Char('G') | KeyCode::End => {
                grid.jump_to(self.files.len().saturating_sub(1), self.files.len());
                self.redraw_all()?;
            }
            KeyCode::Char('?') => {
                self.help_open = true;
                self.redraw_all()?;
            }
            KeyCode::Char('i') => {
                self.info_open = true;
                self.redraw_all()?;
            }
            KeyCode::Char('d') => {
                self.confirm_delete = true;
                self.draw_status_only()?;
            }
            KeyCode::Char(' ') => {
                let path = self.files[grid.selected].clone();
                if !self.marked.insert(path.clone()) {
                    self.marked.remove(&path);
                }
                self.redraw_all()?;
            }
            KeyCode::Char('A') => {
                self.marked.extend(self.files.iter().cloned());
                self.redraw_all()?;
            }
            KeyCode::Char('u') => {
                if !self.marked.is_empty() {
                    self.marked.clear();
                    self.redraw_all()?;
                }
            }
            KeyCode::Char('y') => {
                if self.marked.is_empty() {
                    self.status_msg = Some("nothing marked — Space to mark".into());
                    self.draw_status_only()?;
                } else {
                    self.open_prompt(PromptKind::Export(ExportOp::Copy), String::new())?;
                }
            }
            KeyCode::Char('M') => {
                if self.marked.is_empty() {
                    self.status_msg = Some("nothing marked — Space to mark".into());
                    self.draw_status_only()?;
                } else {
                    self.open_prompt(PromptKind::Export(ExportOp::Move), String::new())?;
                }
            }
            _ => self.handle_common_key(code)?,
        }
        Ok(false)
    }

    pub(super) fn handle_menu_key(&mut self, code: KeyCode) -> Result<bool> {
        let Some(sel) = self.menu_selected else {
            return Ok(false);
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') => {
                self.menu_selected = None;
                self.redraw_all()?;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.menu_selected = Some(next_op_index(sel, 1));
                self.redraw_menu_only()?;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.menu_selected = Some(next_op_index(sel, -1));
                self.redraw_menu_only()?;
            }
            KeyCode::Enter => {
                match &MENU[sel] {
                    MenuItem::Op(_, op) => {
                        let op = *op;
                        self.menu_selected = None;
                        self.apply(op)?;
                    }
                    MenuItem::Resize(_) => {
                        self.menu_selected = None;
                        self.redraw_all()?;
                        self.open_resize_prompt()?;
                    }
                    MenuItem::Separator => {}
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn open_resize_prompt(&mut self) -> Result<()> {
        if self.ensure_loaded().is_ok() {
            self.open_prompt(PromptKind::Resize, String::new())?;
        }
        Ok(())
    }

    /// Keys that behave the same in the grid and the single-image view.
    fn handle_common_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Char('/') => {
                let current = self.filter.clone().unwrap_or_default();
                self.open_prompt(PromptKind::Filter, current)?;
            }
            KeyCode::Char(':') => self.open_prompt(PromptKind::Jump, String::new())?,
            KeyCode::Char('O') => self.cycle_sort()?,
            _ => {}
        }
        Ok(())
    }
}
