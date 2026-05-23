use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use crossterm::{execute, queue};
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, DynamicImage};
use image_webp::WebPDecoder as RawWebPDecoder;

use crate::files::Collection;
use crate::grid::{self, GridState};
use crate::render::Renderer;

#[derive(Copy, Clone, Debug)]
enum EditOp {
    RotateCw,
    RotateCcw,
    Rotate180,
    FlipH,
    FlipV,
}

impl EditOp {
    fn apply(self, img: DynamicImage) -> DynamicImage {
        match self {
            EditOp::RotateCw => img.rotate90(),
            EditOp::RotateCcw => img.rotate270(),
            EditOp::Rotate180 => img.rotate180(),
            EditOp::FlipH => img.fliph(),
            EditOp::FlipV => img.flipv(),
        }
    }
}

enum MenuItem {
    Op(&'static str, EditOp),
    Separator,
}

const MENU: &[MenuItem] = &[
    MenuItem::Op("Rotate 90° CW   (r)", EditOp::RotateCw),
    MenuItem::Op("Rotate 90° CCW  (R)", EditOp::RotateCcw),
    MenuItem::Op("Rotate 180°", EditOp::Rotate180),
    MenuItem::Separator,
    MenuItem::Op("Flip horizontal (f)", EditOp::FlipH),
    MenuItem::Op("Flip vertical   (F)", EditOp::FlipV),
];

struct Frame {
    img: DynamicImage,
    delay: Duration,
}

struct Loaded {
    path: PathBuf,
    /// Static images: the image itself.
    /// Animated images: a copy of the first frame; used for dimensions and as
    /// the fallback when editing freezes the animation.
    img: DynamicImage,
    modified: bool,
    /// None for static images, Some for multi-frame animations (GIF for now).
    frames: Option<Vec<Frame>>,
    frame_idx: usize,
    frame_last: Instant,
}

impl Loaded {
    fn current(&self) -> &DynamicImage {
        match self.frames.as_ref() {
            Some(f) => &f[self.frame_idx].img,
            None => &self.img,
        }
    }

    fn current_delay(&self) -> Duration {
        self.frames
            .as_ref()
            .map(|f| f[self.frame_idx].delay)
            .unwrap_or(Duration::from_secs(3600))
    }
}

const ZOOM_STEP: f32 = 1.25;
const MAX_ZOOM: f32 = 32.0;
const PAN_STEP_FRAC: f32 = 0.1;
const DEFAULT_SLIDESHOW_SECS: f32 = 5.0;
const CROP_STEP: f32 = 0.02;
/// Matches the fit-to-cells logic each renderer uses to lay the image down.
const CELL_ASPECT_H_OVER_W: f64 = 2.0;

#[derive(Copy, Clone, Debug)]
enum ExportOp {
    Copy,
    Move,
}

struct ExportPrompt {
    op: ExportOp,
    buffer: String,
}

#[derive(Copy, Clone, Debug)]
struct CropState {
    /// Selection corners as fractions of the image (0.0 .. 1.0). x0/y0 is the
    /// top-left, x1/y1 the bottom-right; kept ordered so apply_crop doesn't
    /// have to care which side moved.
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl CropState {
    fn new() -> Self {
        Self {
            x0: 0.25,
            y0: 0.25,
            x1: 0.75,
            y1: 0.75,
        }
    }
}

pub struct Viewer {
    files: Vec<PathBuf>,
    index: usize,
    renderer: Box<dyn Renderer>,
    loaded: Option<Loaded>,
    menu_selected: Option<usize>,
    save_as_buffer: Option<String>,
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
    export_prompt: Option<ExportPrompt>,
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
    pub fn from_collection(c: Collection, renderer: Box<dyn Renderer>) -> Result<Self> {
        // When peek was pointed at a directory we land on the grid; with a
        // concrete file we go straight into the single-image view.
        let grid = if c.started_from_directory {
            Some(GridState::new(c.start_index))
        } else {
            None
        };

        Ok(Self {
            files: c.files,
            index: c.start_index,
            renderer,
            loaded: None,
            menu_selected: None,
            save_as_buffer: None,
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
            export_prompt: None,
        })
    }

    pub fn start_slideshow(&mut self, seconds: f32) {
        self.slideshow_interval = Duration::from_secs_f32(seconds.max(0.1));
        self.slideshow_running = true;
        self.slideshow_last = Instant::now();
        // Slideshow is meaningless while sitting on the grid, so jump into the
        // single-image view even if peek was launched against a directory.
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
                        if self.save_as_buffer.is_some() {
                            if self.handle_save_as_key(k.code, k.modifiers)? {
                                break;
                            }
                        } else if self.export_prompt.is_some() {
                            if self.handle_export_prompt_key(k.code, k.modifiers)? {
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
            && self.save_as_buffer.is_none()
            && self.export_prompt.is_none()
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
        self.save_as_buffer.is_none()
            && self.export_prompt.is_none()
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
        if self.animation_active() {
            if let Some(loaded) = self.loaded.as_ref() {
                let elapsed = loaded.frame_last.elapsed();
                best = best.min(loaded.current_delay().saturating_sub(elapsed));
            }
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
        if let Some(loaded) = self.loaded.as_mut() {
            if let Some(frames) = loaded.frames.as_ref() {
                let n = frames.len();
                if n > 1 {
                    loaded.frame_idx = (loaded.frame_idx + 1) % n;
                    loaded.frame_last = Instant::now();
                }
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

    /// Help and Info popups are passive: any key closes them. q still quits.
    fn handle_overlay_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        if matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        self.help_open = false;
        self.info_open = false;
        self.redraw_all()?;
        Ok(matches!(code, KeyCode::Char('q')))
    }

    fn handle_confirm_delete_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
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

    fn handle_crop_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
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

    fn apply_crop(&mut self) -> Result<()> {
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
            let (w, h) = (base.width(), base.height());
            let x0 = (crop.x0.min(crop.x1).clamp(0.0, 1.0) * w as f32).round() as u32;
            let y0 = (crop.y0.min(crop.y1).clamp(0.0, 1.0) * h as f32).round() as u32;
            let x1 = (crop.x0.max(crop.x1).clamp(0.0, 1.0) * w as f32).round() as u32;
            let y1 = (crop.y0.max(crop.y1).clamp(0.0, 1.0) * h as f32).round() as u32;
            let cw = x1.saturating_sub(x0).max(1).min(w - x0);
            let ch = y1.saturating_sub(y0).max(1).min(h - y0);
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

    fn delete_current(&mut self) -> Result<()> {
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

    fn reload_current(&mut self) -> Result<()> {
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

    fn toggle_slideshow(&mut self) -> Result<()> {
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

    /// Returns true when the viewer should exit.
    fn handle_view_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
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

            KeyCode::Char('s') => self.save_current()?,
            KeyCode::Char('S') => {
                self.save_as_buffer = Some(String::new());
                self.draw_status_only()?;
            }

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
            _ => {}
        }
        Ok(false)
    }

    fn handle_grid_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        self.status_msg = None;
        let Some(grid) = self.grid.as_mut() else {
            return Ok(false);
        };
        // Compute current grid columns to translate j/k into row jumps.
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
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
                self.jump_to(target);
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
                    self.export_prompt = Some(ExportPrompt {
                        op: ExportOp::Copy,
                        buffer: String::new(),
                    });
                    self.draw_status_only()?;
                }
            }
            KeyCode::Char('M') => {
                if self.marked.is_empty() {
                    self.status_msg = Some("nothing marked — Space to mark".into());
                    self.draw_status_only()?;
                } else {
                    self.export_prompt = Some(ExportPrompt {
                        op: ExportOp::Move,
                        buffer: String::new(),
                    });
                    self.draw_status_only()?;
                }
            }
            _ => {
                let _ = rows; // silence unused-binding warning in this branch
            }
        }
        Ok(false)
    }

    fn handle_export_prompt_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        match code {
            KeyCode::Esc => {
                self.export_prompt = None;
                self.draw_status_only()?;
            }
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
                self.export_prompt = None;
                self.draw_status_only()?;
            }
            KeyCode::Enter => {
                let prompt = self.export_prompt.take().unwrap();
                let dest = prompt.buffer.trim().to_string();
                if dest.is_empty() {
                    self.status_msg = Some("export cancelled (empty path)".into());
                    self.draw_status_only()?;
                } else {
                    self.execute_export(prompt.op, &dest)?;
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = self.export_prompt.as_mut() {
                    p.buffer.pop();
                }
                self.draw_status_only()?;
            }
            KeyCode::Char(c) => {
                if let Some(p) = self.export_prompt.as_mut() {
                    p.buffer.push(c);
                }
                self.draw_status_only()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn execute_export(&mut self, op: ExportOp, dest: &str) -> Result<()> {
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
            if let Some(loaded) = self.loaded.as_ref() {
                if moved.iter().any(|p| p == &loaded.path) {
                    self.loaded = None;
                }
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

    fn handle_save_as_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        match code {
            KeyCode::Esc => {
                self.save_as_buffer = None;
                self.draw_status_only()?;
            }
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
                self.save_as_buffer = None;
                self.draw_status_only()?;
            }
            KeyCode::Enter => {
                let buf = self.save_as_buffer.take().unwrap_or_default();
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    self.status_msg = Some("save-as cancelled (empty path)".into());
                } else {
                    self.save_to(trimmed);
                }
                self.draw_status_only()?;
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.save_as_buffer.as_mut() {
                    buf.pop();
                }
                self.draw_status_only()?;
            }
            KeyCode::Char(c) => {
                if let Some(buf) = self.save_as_buffer.as_mut() {
                    buf.push(c);
                }
                self.draw_status_only()?;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_menu_key(&mut self, code: KeyCode) -> Result<bool> {
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
                if let MenuItem::Op(_, op) = &MENU[sel] {
                    let op = *op;
                    self.menu_selected = None;
                    self.apply(op)?;
                }
            }
            _ => {}
        }
        Ok(false)
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

    fn apply(&mut self, op: EditOp) -> Result<()> {
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

    fn save_current(&mut self) -> Result<()> {
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
    fn save_to(&mut self, input: &str) {
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

    fn resolve_save_path(&self, input: &str) -> PathBuf {
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

    /// Full repaint: clear, render image, status bar, optionally the menu.
    /// Used when image content or geometry changes (load, edit op, resize, navigate).
    fn redraw_all(&mut self) -> Result<()> {
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
    fn redraw_menu_only(&self) -> Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        if let Some(sel) = self.menu_selected {
            self.draw_menu(cols, rows, sel)?;
        }
        Ok(())
    }

    /// Cheap repaint of only the status row.
    fn draw_status_only(&self) -> Result<()> {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        self.draw_status_bar(cols, rows)
    }

    fn draw_status_bar(&self, cols: u16, rows: u16) -> Result<()> {
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

    fn status_line(&self, cols: u16) -> (Color, Color, String) {
        let width = cols as usize;

        // Save-as input prompt takes priority over everything else.
        if let Some(buf) = self.save_as_buffer.as_ref() {
            let bg = Color::Rgb { r: 60, g: 80, b: 30 };
            let fg = Color::Rgb { r: 230, g: 230, b: 230 };
            let content = format!(" Save as: {buf}_");
            return (bg, fg, pad_or_truncate(&content, width));
        }

        if let Some(prompt) = self.export_prompt.as_ref() {
            let bg = Color::Rgb { r: 30, g: 60, b: 80 };
            let fg = Color::Rgb { r: 230, g: 230, b: 230 };
            let verb = match prompt.op {
                ExportOp::Copy => "Copy",
                ExportOp::Move => "Move",
            };
            let n = self.marked.len();
            let content = format!(" {verb} {n} marked → dir: {}_", prompt.buffer);
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
                " grid · {}/{}{marks}  {}",
                grid.selected + 1,
                self.files.len(),
                name
            );
            let right = " Space mark · y copy · M move · ? help · q quit ";
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
            " {play}{}/{}  {}{}{}{}",
            self.index + 1,
            self.files.len(),
            name,
            flag,
            zoom,
            frame_info,
        );
        let right = " +/- zoom · s save · t grid · ? help · q quit ";
        let pad = width.saturating_sub(left.chars().count() + right.chars().count());
        let line = format!("{left}{}{right}", " ".repeat(pad));
        (bg, fg, line.chars().take(width).collect())
    }

    fn draw_menu(&self, cols: u16, rows: u16, selected: usize) -> Result<()> {
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

    fn draw_crop_overlay(&self, cols: u16, rows: u16, crop: &CropState) -> Result<()> {
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

    fn draw_help_popup(&self, cols: u16, rows: u16) -> Result<()> {
        let lines: Vec<(String, String)> = vec![
            ("n / Space / →".into(), "next image".into()),
            ("p / ←".into(), "previous image".into()),
            ("j k h l".into(), "navigate (pan when zoomed)".into()),
            ("g G".into(), "first / last".into()),
            ("t Tab".into(), "toggle grid".into()),
            (String::new(), String::new()),
            ("Space".into(), "(grid) mark / unmark".into()),
            ("A · u".into(), "(grid) mark all · clear".into()),
            ("y · M".into(), "(grid) copy · move marked to dir".into()),
            (String::new(), String::new()),
            ("+ =".into(), "zoom in".into()),
            ("-".into(), "zoom out".into()),
            ("0".into(), "reset zoom & pan".into()),
            ("i".into(), "info panel".into()),
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
        self.draw_text_box("peek — keymap", &lines, cols, rows)
    }

    fn draw_info_popup(&self, cols: u16, rows: u16) -> Result<()> {
        let path = &self.files[self.index];
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
        let dims = loaded
            .map(|l| format!("{} × {}", l.img.width(), l.img.height()))
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

    fn draw_text_box(
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

/// DEC private mode 2026 — synchronized update. Terminals that don't recognise
/// the escape ignore it silently, so it's safe to send unconditionally.
fn begin_sync() -> Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b[?2026h")?;
    out.flush()?;
    Ok(())
}

fn end_sync() -> Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b[?2026l")?;
    out.flush()?;
    Ok(())
}

/// Compute the cell rectangle the rendered image occupies inside (cols, rows).
/// Mirrors the aspect-preserving fit logic each protocol's renderer applies, so
/// overlays can be drawn relative to the image instead of the full screen.
fn image_cell_rect(img_w: u32, img_h: u32, cols: u16, rows: u16) -> (u16, u16, u16, u16) {
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

fn load_image(path: &std::path::Path) -> Result<Loaded> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());

    // GIF / animated WebP: try multi-frame decode. Falls back to still on a
    // single frame or decoder error (e.g. non-animated GIF89a, still WebP).
    let multi = match ext.as_deref() {
        Some("gif") => load_gif_frames(path).ok(),
        Some("webp") => load_webp_frames(path).ok(),
        _ => None,
    };
    if let Some(frames) = multi {
        if frames.len() > 1 {
            let img = frames[0].img.clone();
            return Ok(Loaded {
                path: path.to_path_buf(),
                img,
                modified: false,
                frames: Some(frames),
                frame_idx: 0,
                frame_last: Instant::now(),
            });
        }
    }

    let img = image::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    Ok(Loaded {
        path: path.to_path_buf(),
        img,
        modified: false,
        frames: None,
        frame_idx: 0,
        frame_last: Instant::now(),
    })
}

fn load_webp_frames(path: &std::path::Path) -> Result<Vec<Frame>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut decoder = RawWebPDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| anyhow::anyhow!("webp decode: {e}"))?;
    if !decoder.is_animated() {
        return Ok(Vec::new());
    }
    let (w, h) = decoder.dimensions();
    let total = decoder.num_frames() as usize;
    let buf_size = decoder
        .output_buffer_size()
        .unwrap_or((w as usize) * (h as usize) * 4);

    let mut frames = Vec::with_capacity(total);
    for _ in 0..total {
        let mut buf = vec![0u8; buf_size];
        let delay_ms = decoder
            .read_frame(&mut buf)
            .map_err(|e| anyhow::anyhow!("webp frame: {e}"))?;
        // image-webp emits RGBA8 for animated frames.
        let img = image::RgbaImage::from_raw(w, h, buf)
            .ok_or_else(|| anyhow::anyhow!("webp frame size mismatch"))?;
        let delay = Duration::from_millis(delay_ms.max(20) as u64);
        frames.push(Frame {
            img: DynamicImage::ImageRgba8(img),
            delay,
        });
    }
    Ok(frames)
}

fn load_gif_frames(path: &std::path::Path) -> Result<Vec<Frame>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let decoder = GifDecoder::new(std::io::BufReader::new(file))?;
    let raw_frames = decoder.into_frames().collect_frames()?;

    Ok(raw_frames
        .into_iter()
        .map(|f| {
            let (n, d) = f.delay().numer_denom_ms();
            let ms = if d == 0 {
                100.0
            } else {
                n as f64 / d as f64
            };
            // Most browsers clamp GIF frame delays at 20ms (50fps) to keep CPU
            // sane; do the same so 0-delay loops don't pin the redraw thread.
            let delay = Duration::from_millis(ms.round() as u64).max(Duration::from_millis(20));
            Frame {
                img: DynamicImage::ImageRgba8(f.into_buffer()),
                delay,
            }
        })
        .collect())
}

fn resolve_dest_dir(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if trimmed == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(trimmed)
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

fn truncate_middle(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max || max < 4 {
        return s.chars().take(max).collect();
    }
    let keep_right = (max - 1) / 2;
    let keep_left = max - 1 - keep_right;
    let left: String = s.chars().take(keep_left).collect();
    let right: String = s.chars().skip(count - keep_right).collect();
    format!("{left}…{right}")
}

fn pad_or_truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        s.chars().take(width).collect()
    } else {
        let mut out = String::from(s);
        out.extend(std::iter::repeat(' ').take(width - count));
        out
    }
}

fn first_op_index() -> usize {
    MENU.iter()
        .position(|item| matches!(item, MenuItem::Op(_, _)))
        .unwrap_or(0)
}

fn next_op_index(current: usize, delta: i32) -> usize {
    let len = MENU.len() as i32;
    let mut i = current as i32;
    loop {
        i += delta;
        if i < 0 {
            i = len - 1;
        } else if i >= len {
            i = 0;
        }
        if let MenuItem::Op(_, _) = MENU[i as usize] {
            return i as usize;
        }
        if i == current as i32 {
            return current;
        }
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
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
