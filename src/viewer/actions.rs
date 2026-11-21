use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use super::edit::EditOp;
use super::{ExportOp, Viewer};
use crate::util::resolve_dest_dir;

impl Viewer {
    pub(super) fn apply_crop(&mut self) -> Result<()> {
        let Some(crop) = self.crop.take() else {
            return Ok(());
        };
        if let Some(mut loaded) = self.loaded.take() {
            let base = match loaded.frames {
                Some(mut frames) => {
                    let idx = loaded.frame_idx.min(frames.len().saturating_sub(1));
                    frames.swap_remove(idx).img
                }
                None => loaded.img,
            };
            let (x0, y0, cw, ch) = crop.pixel_rect(base.width(), base.height());
            let cropped = base.crop_imm(x0, y0, cw, ch);
            self.status_msg = Some(format!("cropped to {cw}×{ch}"));
            loaded.img = cropped;
            loaded.frames = None;
            loaded.frame_idx = 0;
            loaded.modified = true;
            self.loaded = Some(loaded);
        }
        self.reset_view_state();
        self.redraw_all()?;
        Ok(())
    }

    pub(super) fn delete_current(&mut self) -> Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        // In grid mode the target is the selected thumb, otherwise the open image.
        let target_idx = self.grid.as_ref().map(|g| g.selected).unwrap_or(self.index);
        let path = self.files[target_idx].clone();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        if let Err(e) = std::fs::remove_file(&path) {
            self.status_msg = Some(format!("delete failed: {e}"));
            self.draw_status_only()?;
            return Ok(());
        }
        self.files.remove(target_idx);
        self.marked.remove(&path);
        self.preload_cache.remove(&path);

        if let Some(grid) = self.grid.as_mut() {
            grid.forget(&path);
            if !self.files.is_empty() && grid.selected >= self.files.len() {
                grid.selected = self.files.len() - 1;
            }
        }
        if target_idx < self.index {
            self.index -= 1;
        }
        if !self.files.is_empty() && self.index >= self.files.len() {
            self.index = self.files.len() - 1;
        }

        self.loaded = None;
        self.reset_view_state();

        if self.files.is_empty() {
            self.status_msg = Some(format!("deleted {name} — no images left, exiting"));
            self.draw_status_only()?;
        } else {
            self.schedule_preloads();
            self.status_msg = Some(format!("deleted {name}"));
            self.redraw_all()?;
        }
        Ok(())
    }

    pub(super) fn reload_current(&mut self) -> Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        self.loaded = None;
        match self.ensure_loaded() {
            Ok(()) => self.status_msg = Some("reloaded".into()),
            Err(e) => self.status_msg = Some(format!("reload failed: {e}")),
        }
        self.redraw_all()?;
        Ok(())
    }

    pub(super) fn toggle_slideshow(&mut self) -> Result<()> {
        self.slideshow_running = !self.slideshow_running;
        if self.slideshow_running {
            self.slideshow_last = Instant::now();
            self.status_msg = Some(format!(
                "slideshow ▶ every {:.1}s — P to pause",
                self.slideshow_interval.as_secs_f32()
            ));
        } else {
            self.status_msg = Some("slideshow paused".into());
        }
        self.draw_status_only()?;
        Ok(())
    }

    pub(super) fn execute_export(&mut self, op: ExportOp, dest: &str) -> Result<()> {
        let target_dir = resolve_dest_dir(dest);
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            self.status_msg = Some(format!("create dir failed: {e}"));
            self.draw_status_only()?;
            return Ok(());
        }

        let label = match op {
            ExportOp::Copy => "copied",
            ExportOp::Move => "moved",
        };
        let marked: Vec<PathBuf> = self.marked.iter().cloned().collect();
        let total = marked.len();
        let mut ok = 0;
        let mut moved: Vec<PathBuf> = Vec::new();
        let mut successful: HashSet<PathBuf> = HashSet::new();
        let mut errors: Vec<String> = Vec::new();

        for src in &marked {
            let Some(name) = src.file_name() else { continue };
            let dst = target_dir.join(name);
            if dst == *src {
                continue;
            }
            let result = match op {
                ExportOp::Copy => std::fs::copy(src, &dst).map(|_| ()),
                ExportOp::Move => std::fs::rename(src, &dst).or_else(|_| {
                    // Cross-filesystem rename fails on macOS/Linux; fall back
                    // to copy + remove so the user doesn't see a confusing
                    // EXDEV error when crossing volumes.
                    std::fs::copy(src, &dst).and_then(|_| std::fs::remove_file(src))
                }),
            };
            match result {
                Ok(()) => {
                    ok += 1;
                    successful.insert(src.clone());
                    if matches!(op, ExportOp::Move) {
                        moved.push(src.clone());
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", name.to_string_lossy())),
            }
        }

        // Drop marks of files we actually exported; failures stay marked so
        // the user can retry with a different destination.
        self.marked.retain(|p| !successful.contains(p));

        // For Move: rebuild file list (paths are gone from the source dir).
        if !moved.is_empty() {
            let moved_set: HashSet<&PathBuf> = moved.iter().collect();
            let view_path = self.files.get(self.index).cloned();
            let grid_sel_path = self
                .grid
                .as_ref()
                .and_then(|g| self.files.get(g.selected).cloned());

            self.files.retain(|p| !moved_set.contains(p));

            self.index = view_path
                .as_ref()
                .and_then(|p| self.files.iter().position(|f| f == p))
                .unwrap_or_else(|| self.index.min(self.files.len().saturating_sub(1)));

            if let Some(grid) = self.grid.as_mut() {
                let new_sel = grid_sel_path
                    .as_ref()
                    .and_then(|p| self.files.iter().position(|f| f == p))
                    .unwrap_or_else(|| grid.selected.min(self.files.len().saturating_sub(1)));
                grid.selected = new_sel;
                for p in &moved {
                    grid.forget(p);
                }
            }

            // The currently displayed image may have been moved.
            if let Some(loaded) = self.loaded.as_ref()
                && moved.iter().any(|p| p == &loaded.path) {
                    self.loaded = None;
                }
            self.preload_cache
                .retain(|p, _| !moved.iter().any(|m| m == p));
        }

        self.status_msg = Some(if errors.is_empty() {
            format!("{ok}/{total} {label} to {}", target_dir.display())
        } else {
            format!(
                "{ok}/{total} {label} to {} ({} error{})",
                target_dir.display(),
                errors.len(),
                if errors.len() == 1 { "" } else { "s" },
            )
        });

        self.redraw_all()?;
        Ok(())
    }

    pub(super) fn apply(&mut self, op: EditOp) -> Result<()> {
        self.ensure_loaded()?;
        if let Some(mut loaded) = self.loaded.take() {
            // Editing an animation freezes it to the currently displayed frame
            // — saving a multi-frame image with edits applied isn't supported,
            // so we collapse to a single frame instead.
            let base = match loaded.frames.as_ref() {
                Some(f) => f[loaded.frame_idx].img.clone(),
                None => loaded.img,
            };
            loaded.img = op.apply(base);
            loaded.frames = None;
            loaded.frame_idx = 0;
            loaded.modified = true;
            self.loaded = Some(loaded);
        }
        // Rotate changes dimensions; safest to reset zoom/pan.
        self.reset_view_state();
        self.redraw_all()?;
        Ok(())
    }

    pub(super) fn save_current(&mut self) -> Result<()> {
        let Some(loaded) = self.loaded.as_ref() else {
            self.status_msg = Some("nothing to save".into());
            self.draw_status_only()?;
            return Ok(());
        };
        if !loaded.modified {
            self.status_msg = Some("no changes".into());
            self.draw_status_only()?;
            return Ok(());
        }
        let path = loaded.path.clone();
        let display = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        match loaded.img.save(&path) {
            Ok(()) => {
                if let Some(l) = self.loaded.as_mut() {
                    l.modified = false;
                }
                self.status_msg = Some(format!("saved: {display}"));
            }
            Err(e) => self.status_msg = Some(format!("save failed: {e}")),
        }
        self.draw_status_only()?;
        Ok(())
    }

    /// Writes the in-memory image to `input` without changing the active file.
    /// `input` may be absolute or relative to the directory being browsed.
    pub(super) fn save_to(&mut self, input: &str) {
        let Some(loaded) = self.loaded.as_ref() else {
            self.status_msg = Some("nothing to save".into());
            return;
        };
        let target = self.resolve_save_path(input);
        match loaded.img.save(&target) {
            Ok(()) => {
                let display = target
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| target.display().to_string());
                self.status_msg = Some(format!("wrote: {display}"));
            }
            Err(e) => self.status_msg = Some(format!("save-as failed: {e}")),
        }
    }

    pub(super) fn resolve_save_path(&self, input: &str) -> PathBuf {
        let p = PathBuf::from(input);
        if p.is_absolute() {
            return p;
        }
        let base = self.files[self.index]
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join(p)
    }
}
