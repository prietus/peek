use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use image::DynamicImage;
use image::imageops::FilterType;

use super::edit::{EditOp, parse_resize, save_image};
use super::load::UndoState;
use super::term::with_terminal_released;
use crate::external;
use super::{ExportOp, Viewer};
use crate::util::resolve_dest_dir;

/// How many edits `u` can step back through per image.
const MAX_UNDO: usize = 10;

impl Viewer {
    pub(super) fn apply_crop(&mut self) -> Result<()> {
        let Some(crop) = self.crop.take() else {
            return Ok(());
        };
        let (iw, ih) = match self.loaded.as_ref() {
            Some(l) => (l.current().width(), l.current().height()),
            None => return self.redraw_all(),
        };
        let (x0, y0, cw, ch) = crop.pixel_rect(iw, ih);
        self.edit_with(format!("cropped to {cw}×{ch}"), true, |img| {
            img.crop_imm(x0, y0, cw, ch)
        })
    }

    pub(super) fn delete_current(&mut self) -> Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        // In grid mode the target is the selected thumb, otherwise the open image.
        let target_idx = self.target_index();
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
        self.reset_view_state();
        self.remove_paths(&HashSet::from([path]));

        if self.files.is_empty() {
            self.status_msg = Some(format!("deleted {name} — no images left, exiting"));
            self.draw_status_only()?;
        } else {
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

        // Moved files are gone from where we were browsing.
        self.remove_paths(&moved.into_iter().collect());

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
        self.edit_with(op.describe(), op.changes_geometry(), |img| op.apply(img))
    }

    pub(super) fn resize_to(&mut self, spec: &str) -> Result<()> {
        let Some((w, h)) = self
            .loaded
            .as_ref()
            .map(|l| (l.current().width(), l.current().height()))
        else {
            return Ok(());
        };
        match parse_resize(spec, w, h) {
            Some((nw, nh)) => self.edit_with(format!("resized {w}×{h} → {nw}×{nh}"), true, |img| {
                img.resize_exact(nw, nh, FilterType::Lanczos3)
            }),
            None => {
                self.status_msg =
                    Some(format!("bad size \"{spec}\" — try 50%, 800, x600, 800x600 or 800x600!"));
                self.draw_status_only()
            }
        }
    }

    /// Run an edit on the image on screen, recording the previous state for
    /// undo. Editing an animation freezes it to the displayed frame; undo
    /// brings the animation back.
    fn edit_with(
        &mut self,
        description: String,
        reset_view: bool,
        f: impl FnOnce(&DynamicImage) -> DynamicImage,
    ) -> Result<()> {
        if let Err(e) = self.ensure_loaded() {
            self.status_msg = Some(format!("cannot edit: {e}"));
            return self.draw_status_only();
        }
        let Some(loaded) = self.loaded.as_mut() else {
            return Ok(());
        };
        let edited = f(loaded.current());
        let previous = std::mem::replace(&mut loaded.img, edited);
        loaded.undo.push(UndoState {
            img: previous,
            frames: loaded.frames.take(),
            frame_idx: loaded.frame_idx,
            modified: loaded.modified,
        });
        if loaded.undo.len() > MAX_UNDO {
            loaded.undo.remove(0);
        }
        loaded.frame_idx = 0;
        loaded.modified = true;
        self.status_msg = Some(description);
        if reset_view {
            self.reset_view_state();
        }
        self.redraw_all()
    }

    pub(super) fn undo(&mut self) -> Result<()> {
        let Some(loaded) = self.loaded.as_mut() else {
            return Ok(());
        };
        let Some(state) = loaded.undo.pop() else {
            self.status_msg = Some("nothing to undo".into());
            return self.draw_status_only();
        };
        loaded.img = state.img;
        loaded.frames = state.frames;
        loaded.frame_idx = state.frame_idx;
        loaded.frame_last = Instant::now();
        loaded.modified = state.modified;
        let left = loaded.undo.len();
        self.status_msg = Some(format!("undone ({left} more)"));
        self.reset_view_state();
        self.redraw_all()
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
        match save_image(&loaded.img, &path) {
            Ok(()) => {
                if let Some(l) = self.loaded.as_mut() {
                    l.modified = false;
                    // The file on disk now matches the current image, so
                    // every earlier state differs from it.
                    for u in &mut l.undo {
                        u.modified = true;
                    }
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
        match save_image(&loaded.img, &target) {
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

    /// Path of the file the user is acting on, or None when the list is empty.
    fn target_path(&self) -> Option<PathBuf> {
        self.files.get(self.target_index()).cloned()
    }

    fn report(&mut self, msg: String) -> Result<()> {
        self.status_msg = Some(msg);
        self.draw_status_only()
    }

    pub(super) fn open_external(&mut self) -> Result<()> {
        let Some(path) = self.target_path() else {
            return Ok(());
        };
        let cmd = self
            .config
            .open_command
            .clone()
            .unwrap_or_else(|| external::default_open_command().to_string());
        let msg = match external::spawn_background(&cmd, &path) {
            Ok(()) => format!("opened with {cmd}"),
            Err(e) => format!("open failed: {e:#}"),
        };
        self.report(msg)
    }

    pub(super) fn copy_path(&mut self) -> Result<()> {
        let Some(path) = self.target_path() else {
            return Ok(());
        };
        let full = std::fs::canonicalize(&path).unwrap_or(path);
        let msg = match external::copy_text(&full.to_string_lossy()) {
            Ok(how) => format!("copied path via {how}"),
            Err(e) => format!("copy failed: {e:#}"),
        };
        self.report(msg)
    }

    /// Copy the image as shown, including unsaved edits. In the grid the
    /// selected file is decoded from disk.
    pub(super) fn copy_image(&mut self) -> Result<()> {
        let Some(path) = self.target_path() else {
            return Ok(());
        };
        let from_screen = self.grid.is_none()
            && self.loaded.as_ref().is_some_and(|l| l.path == path);
        let result = if from_screen {
            external::copy_image(self.loaded.as_ref().unwrap().current())
        } else {
            crate::decode::open(&path).and_then(|img| external::copy_image(&img))
        };
        let msg = match result {
            Ok(how) => format!("copied image via {how}"),
            Err(e) => format!("copy failed: {e:#}"),
        };
        self.report(msg)
    }

    pub(super) fn set_wallpaper(&mut self) -> Result<()> {
        let Some(path) = self.target_path() else {
            return Ok(());
        };
        let unsaved = self
            .loaded
            .as_ref()
            .is_some_and(|l| l.path == path && l.modified);
        let msg = match external::set_wallpaper(&path, self.config.wallpaper_command.as_deref()) {
            Ok(how) if unsaved => format!("wallpaper set via {how} (from disk — save to include edits)"),
            Ok(how) => format!("wallpaper set via {how}"),
            Err(e) => format!("wallpaper failed: {e:#}"),
        };
        self.report(msg)
    }

    /// Run a `cmd.<key>` from the config. Commands starting with `!` take
    /// over the terminal until they exit; the rest run in the background.
    pub(super) fn run_user_command(&mut self, template: &str) -> Result<()> {
        let Some(path) = self.target_path() else {
            return Ok(());
        };
        if let Some(fg) = template.strip_prefix('!') {
            let fg = fg.trim();
            let status = with_terminal_released(|| {
                let status = external::run_foreground(fg, &path);
                // Give the user a chance to read the output before the
                // viewer repaints over it.
                print!("\n[ipeek] press Enter to return ");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                let _ = std::io::stdin().read_line(&mut String::new());
                status
            })?;
            self.status_msg = Some(match status {
                Ok(s) if s.success() => format!("ran: {fg}"),
                Ok(s) => format!("`{fg}` exited with {s}"),
                Err(e) => format!("failed: {e:#}"),
            });
            // The command may have renamed, deleted or edited the file.
            self.refresh_after_command(&path);
            return self.redraw_all();
        }
        let msg = match external::spawn_background(template, &path) {
            Ok(()) => format!("started: {template}"),
            Err(e) => format!("failed: {e:#}"),
        };
        self.report(msg)
    }

    fn refresh_after_command(&mut self, path: &std::path::Path) {
        if !path.exists() {
            self.remove_paths(&HashSet::from([path.to_path_buf()]));
            return;
        }
        let unmodified = self.loaded.as_ref().is_some_and(|l| l.path == path && !l.modified);
        if unmodified {
            self.loaded = None;
        }
        self.preload_cache.remove(path);
        if let Some(grid) = self.grid.as_mut() {
            grid.forget(path);
        }
    }
}
