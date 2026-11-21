mod actions;
mod draw;
mod edit;
mod keys;
mod load;
mod prompt;
mod term;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use image::DynamicImage;

use crate::files::{self, Collection, SortMode};
use crate::grid::GridState;
use crate::render::Renderer;

use edit::CropState;
use load::{Loaded, load_image};
use prompt::Prompt;
use term::TerminalGuard;

const ZOOM_STEP: f32 = 1.25;
const MAX_ZOOM: f32 = 32.0;
const PAN_STEP_FRAC: f32 = 0.1;
const DEFAULT_SLIDESHOW_SECS: f32 = 5.0;
pub(super) const CROP_STEP: f32 = 0.02;
/// Matches the fit-to-cells logic each renderer uses to lay the image down.
pub(super) const CELL_ASPECT_H_OVER_W: f64 = 2.0;


#[derive(Copy, Clone, Debug)]
enum ExportOp {
    Copy,
    Move,
}


pub struct Viewer {
    /// Every file ipeek knows about, in the current sort order.
    all_files: Vec<PathBuf>,
    /// The files currently shown: `all_files` narrowed by `filter`.
    files: Vec<PathBuf>,
    filter: Option<String>,
    sort: SortMode,
    index: usize,
    renderer: Box<dyn Renderer>,
    loaded: Option<Loaded>,
    menu_selected: Option<usize>,
    prompt: Option<Prompt>,
    status_msg: Option<String>,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    grid: Option<GridState>,
    help_open: bool,
    info_open: bool,
    confirm_delete: bool,
    crop: Option<CropState>,
    slideshow_interval: Duration,
    slideshow_running: bool,
    slideshow_last: Instant,
    preloader: Preloader,
    preload_cache: HashMap<PathBuf, Loaded>,
    marked: HashSet<PathBuf>,
}

struct PreloadResult {
    path: PathBuf,
    result: Result<Loaded>,
}

/// Background worker that decodes images off the main thread. The main loop
/// requests neighbouring files on every navigate and drains finished results
/// on every event-loop iteration; cache hits make navigation feel instant.
struct Preloader {
    tx_req: Sender<PathBuf>,
    rx_res: Receiver<PreloadResult>,
    in_flight: HashSet<PathBuf>,
}

impl Preloader {
    fn spawn() -> Self {
        let (tx_req, rx_req) = mpsc::channel::<PathBuf>();
        let (tx_res, rx_res) = mpsc::channel::<PreloadResult>();
        thread::spawn(move || {
            while let Ok(path) = rx_req.recv() {
                let result = load_image(&path);
                if tx_res
                    .send(PreloadResult { path, result })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            tx_req,
            rx_res,
            in_flight: HashSet::new(),
        }
    }

    fn request(&mut self, path: PathBuf) {
        if self.in_flight.insert(path.clone()) {
            let _ = self.tx_req.send(path);
        }
    }

    fn drain(&mut self) -> Vec<PreloadResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx_res.try_recv() {
            self.in_flight.remove(&r.path);
            out.push(r);
        }
        out
    }
}

impl Viewer {
    pub fn from_collection(
        c: Collection,
        renderer: Box<dyn Renderer>,
        sort: SortMode,
    ) -> Result<Self> {
        // When ipeek was pointed at a directory we land on the grid; with a
        // concrete file we go straight into the single-image view.
        let grid = if c.started_from_directory {
            Some(GridState::new(c.start_index))
        } else {
            None
        };

        Ok(Self {
            all_files: c.files.clone(),
            files: c.files,
            filter: None,
            sort,
            index: c.start_index,
            renderer,
            loaded: None,
            menu_selected: None,
            prompt: None,
            status_msg: None,
            zoom: 1.0,
            pan_x: 0.5,
            pan_y: 0.5,
            grid,
            help_open: false,
            info_open: false,
            confirm_delete: false,
            crop: None,
            slideshow_interval: Duration::from_secs_f32(DEFAULT_SLIDESHOW_SECS),
            slideshow_running: false,
            slideshow_last: Instant::now(),
            preloader: Preloader::spawn(),
            preload_cache: HashMap::new(),
            marked: HashSet::new(),
        })
    }

    pub fn start_slideshow(&mut self, seconds: f32) {
        self.slideshow_interval = Duration::from_secs_f32(seconds.max(0.1));
        self.slideshow_running = true;
        self.slideshow_last = Instant::now();
        // Slideshow is meaningless while sitting on the grid, so jump into the
        // single-image view even if ipeek was launched against a directory.
        self.grid = None;
    }

    pub fn run(&mut self) -> Result<()> {
        let _guard = TerminalGuard::enter()?;
        self.schedule_preloads();
        self.redraw_all()?;

        loop {
            if self.files.is_empty() {
                break;
            }
            self.drain_preloads();
            let timeout = self.next_tick_timeout();
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if self.prompt.is_some() {
                            if self.handle_prompt_key(k.code, k.modifiers)? {
                                break;
                            }
                        } else if self.confirm_delete {
                            if self.handle_confirm_delete_key(k.code, k.modifiers)? {
                                break;
                            }
                        } else if self.crop.is_some() {
                            if self.handle_crop_key(k.code, k.modifiers)? {
                                break;
                            }
                        } else if self.help_open || self.info_open {
                            if self.handle_overlay_key(k.code, k.modifiers)? {
                                break;
                            }
                        } else if self.menu_selected.is_some() {
                            if self.handle_menu_key(k.code)? {
                                break;
                            }
                        } else if self.grid.is_some() {
                            if self.handle_grid_key(k.code, k.modifiers)? {
                                break;
                            }
                        } else if self.handle_view_key(k.code, k.modifiers)? {
                            break;
                        }
                    }
                    Event::Resize(_, _) => self.redraw_all()?,
                    _ => {}
                }
            } else {
                self.on_idle_tick()?;
            }
        }
        Ok(())
    }

    fn slideshow_active(&self) -> bool {
        self.slideshow_running
            && self.prompt.is_none()
            && !self.confirm_delete
            && !self.help_open
            && !self.info_open
            && self.menu_selected.is_none()
            && self.grid.is_none()
            && self.crop.is_none()
    }

    /// Animation keeps ticking through help/info popups (they're transparent
    /// overlays), but pauses in modal states that take over the screen.
    fn animation_active(&self) -> bool {
        self.prompt.is_none()
            && !self.confirm_delete
            && self.menu_selected.is_none()
            && self.grid.is_none()
            && self.crop.is_none()
            && self
                .loaded
                .as_ref()
                .map(|l| l.frames.is_some())
                .unwrap_or(false)
    }

    fn next_tick_timeout(&self) -> Duration {
        let mut best = Duration::from_secs(3600);
        if self.slideshow_active() {
            let elapsed = self.slideshow_last.elapsed();
            best = best.min(self.slideshow_interval.saturating_sub(elapsed));
        }
        if self.animation_active()
            && let Some(loaded) = self.loaded.as_ref() {
                let elapsed = loaded.frame_last.elapsed();
                best = best.min(loaded.current_delay().saturating_sub(elapsed));
            }
        best
    }

    fn on_idle_tick(&mut self) -> Result<()> {
        if self.slideshow_active() && self.slideshow_last.elapsed() >= self.slideshow_interval {
            self.slideshow_last = Instant::now();
            self.navigate_wrap(1);
            return Ok(());
        }
        if self.animation_active() {
            let due = self
                .loaded
                .as_ref()
                .map(|l| l.frame_last.elapsed() >= l.current_delay())
                .unwrap_or(false);
            if due {
                self.advance_frame()?;
            }
        }
        Ok(())
    }

    fn advance_frame(&mut self) -> Result<()> {
        if let Some(loaded) = self.loaded.as_mut()
            && let Some(frames) = loaded.frames.as_ref() {
                let n = frames.len();
                if n > 1 {
                    loaded.frame_idx = (loaded.frame_idx + 1) % n;
                    loaded.frame_last = Instant::now();
                }
            }
        self.redraw_all()?;
        Ok(())
    }

    fn navigate_wrap(&mut self, delta: i32) {
        if self.files.is_empty() {
            return;
        }
        let len = self.files.len() as i32;
        let new = (self.index as i32 + delta).rem_euclid(len) as usize;
        if new != self.index {
            self.index = new;
            self.loaded = None;
            self.reset_view_state();
            self.schedule_preloads();
            let _ = self.redraw_all();
        }
    }

    fn navigate(&mut self, delta: i32) {
        let new_index = (self.index as i32 + delta).clamp(0, self.files.len() as i32 - 1);
        if new_index as usize != self.index {
            self.index = new_index as usize;
            self.loaded = None;
            self.reset_view_state();
            self.schedule_preloads();
            let _ = self.redraw_all();
        }
    }

    fn jump_to(&mut self, target: usize) {
        if target != self.index {
            self.index = target;
            self.loaded = None;
            self.reset_view_state();
            self.schedule_preloads();
            let _ = self.redraw_all();
        }
    }

    fn reset_view_state(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.5;
        self.pan_y = 0.5;
    }

    fn zoom_in(&mut self) -> Result<()> {
        self.zoom = (self.zoom * ZOOM_STEP).min(MAX_ZOOM);
        self.redraw_all()
    }

    fn zoom_out(&mut self) -> Result<()> {
        self.zoom = (self.zoom / ZOOM_STEP).max(1.0);
        // Snap back to the fit view when reaching 1.0 to avoid float drift.
        if self.zoom < 1.001 {
            self.reset_view_state();
        } else {
            self.clamp_pan();
        }
        self.redraw_all()
    }

    fn reset_view(&mut self) -> Result<()> {
        self.reset_view_state();
        self.redraw_all()
    }

    fn pan(&mut self, dx: f32, dy: f32) -> Result<()> {
        if self.zoom <= 1.0 {
            return Ok(());
        }
        let step = PAN_STEP_FRAC / self.zoom;
        self.pan_x += dx * step;
        self.pan_y += dy * step;
        self.clamp_pan();
        self.redraw_all()
    }

    fn clamp_pan(&mut self) {
        let half_view = 0.5 / self.zoom;
        let lo = half_view.min(0.5);
        let hi = (1.0 - half_view).max(0.5);
        self.pan_x = self.pan_x.clamp(lo, hi);
        self.pan_y = self.pan_y.clamp(lo, hi);
    }

    /// Returns a cropped sub-image when zoom > 1, otherwise None (render source as-is).
    fn view_image(&self) -> Option<DynamicImage> {
        if self.zoom <= 1.001 {
            return None;
        }
        let loaded = self.loaded.as_ref()?;
        let img = loaded.current();
        let (iw, ih) = (img.width(), img.height());
        let crop_w = ((iw as f32 / self.zoom).round() as u32).max(1).min(iw);
        let crop_h = ((ih as f32 / self.zoom).round() as u32).max(1).min(ih);
        let center_x = self.pan_x * iw as f32;
        let center_y = self.pan_y * ih as f32;
        let left = (center_x - crop_w as f32 / 2.0)
            .max(0.0)
            .min((iw - crop_w) as f32) as u32;
        let top = (center_y - crop_h as f32 / 2.0)
            .max(0.0)
            .min((ih - crop_h) as f32) as u32;
        Some(img.crop_imm(left, top, crop_w, crop_h))
    }

    /// Index of the file the user is acting on: the grid selection when the
    /// grid is open, otherwise the image on screen.
    fn target_index(&self) -> usize {
        self.grid.as_ref().map(|g| g.selected).unwrap_or(self.index)
    }

    /// Move to `idx` in whichever mode is active.
    fn go_to(&mut self, idx: usize) {
        if let Some(grid) = self.grid.as_mut() {
            grid.jump_to(idx, self.files.len());
        } else {
            self.jump_to(idx);
        }
    }

    /// Recompute `files` from `all_files` + `filter`, keeping the open image
    /// and the grid selection on the same paths when they survive.
    fn rebuild_listing(&mut self) {
        let view_path = self.files.get(self.index).cloned();
        let sel_path = self.grid.as_ref().and_then(|g| self.files.get(g.selected).cloned());

        let query = self.filter.as_deref().unwrap_or("");
        self.files = self
            .all_files
            .iter()
            .filter(|p| files::matches_filter(p, query))
            .cloned()
            .collect();
        // A filter that empties the listing (e.g. after deleting the last
        // match) would leave nothing to show, so fall back to everything.
        if self.files.is_empty() && !self.all_files.is_empty() {
            self.filter = None;
            self.files = self.all_files.clone();
        }

        let last = self.files.len().saturating_sub(1);
        let find = |p: &Option<PathBuf>| p.as_ref().and_then(|p| self.files.iter().position(|f| f == p));
        let new_index = find(&view_path).unwrap_or(self.index.min(last));
        let new_sel = find(&sel_path);
        if let Some(grid) = self.grid.as_mut() {
            grid.selected = new_sel.unwrap_or(grid.selected.min(last));
        }
        if self.files.get(new_index) != view_path.as_ref() {
            self.loaded = None;
            self.reset_view_state();
        }
        self.index = new_index;
        if !self.files.is_empty() {
            self.schedule_preloads();
        }
    }

    /// Forget files that no longer exist at their old path (deleted/moved).
    fn remove_paths(&mut self, gone: &HashSet<PathBuf>) {
        if gone.is_empty() {
            return;
        }
        // Keep the cursor near where it was rather than jumping to the start.
        let view_path = self.files.get(self.index).cloned();
        if view_path.as_ref().is_some_and(|p| gone.contains(p)) {
            self.loaded = None;
        }
        self.all_files.retain(|p| !gone.contains(p));
        self.marked.retain(|p| !gone.contains(p));
        self.preload_cache.retain(|p, _| !gone.contains(p));
        if let Some(grid) = self.grid.as_mut() {
            for p in gone {
                grid.forget(p);
            }
        }
        // Surviving paths keep their position; if the open image itself
        // vanished, the index stays put so the next file slides into view.
        self.rebuild_listing();
    }

    fn set_filter(&mut self, query: &str) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            self.filter = None;
            self.status_msg = Some(format!("filter cleared — {} files", self.all_files.len()));
        } else if !self.all_files.iter().any(|p| files::matches_filter(p, query)) {
            self.status_msg = Some(format!("no files match \"{query}\""));
            return self.draw_status_only();
        } else {
            self.filter = Some(query.to_string());
        }
        self.rebuild_listing();
        if let Some(f) = self.filter.as_ref() {
            self.status_msg = Some(format!(
                "filter \"{f}\": {} of {} files",
                self.files.len(),
                self.all_files.len()
            ));
        }
        self.redraw_all()
    }

    fn cycle_sort(&mut self) -> Result<()> {
        self.sort = self.sort.next();
        files::sort_files(&mut self.all_files, self.sort);
        self.rebuild_listing();
        self.status_msg = Some(format!("sorted by {}", self.sort.label()));
        self.redraw_all()
    }

    fn ensure_loaded(&mut self) -> Result<()> {
        let path = self.files[self.index].clone();
        if self.loaded.as_ref().map(|l| l.path == path).unwrap_or(false) {
            return Ok(());
        }
        // Fast path: the background worker already decoded this file.
        if let Some(l) = self.preload_cache.remove(&path) {
            self.loaded = Some(l);
            return Ok(());
        }
        self.loaded = Some(load_image(&path)?);
        Ok(())
    }

    /// Ask the worker to decode the previous + next files (if any) and evict
    /// preloaded items that are no longer neighbours of the current index.
    fn schedule_preloads(&mut self) {
        let len = self.files.len();
        if len < 2 {
            self.preload_cache.clear();
            return;
        }

        let mut neighbours: HashSet<PathBuf> = HashSet::new();
        if self.index + 1 < len {
            neighbours.insert(self.files[self.index + 1].clone());
        }
        if self.index > 0 {
            neighbours.insert(self.files[self.index - 1].clone());
        }

        // Drop the current image from the preload cache if it leaked in there
        // (e.g. after a delete shifting indices); self.loaded is the source of
        // truth for what's on screen.
        let current = self.files[self.index].clone();
        self.preload_cache
            .retain(|p, _| neighbours.contains(p) && *p != current);

        for path in neighbours {
            if !self.preload_cache.contains_key(&path) {
                self.preloader.request(path);
            }
        }
    }

    fn drain_preloads(&mut self) {
        for r in self.preloader.drain() {
            if let Ok(loaded) = r.result {
                self.preload_cache.insert(r.path, loaded);
            }
            // Decode errors are silently dropped; ensure_loaded will retry
            // synchronously and surface the error on the foreground path.
        }
    }

}