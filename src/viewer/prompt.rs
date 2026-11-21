//! Single-line text prompts shown in the status bar (save-as, copy/move
//! destination, filter, jump-to-index). One prompt can be open at a time;
//! while it is, every key goes to the prompt.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::style::Color;

use super::{ExportOp, Viewer};

#[derive(Copy, Clone, Debug)]
pub(super) enum PromptKind {
    SaveAs,
    Export(ExportOp),
    Filter,
    Jump,
}

pub(super) struct Prompt {
    pub(super) kind: PromptKind,
    pub(super) buffer: String,
}

impl Viewer {
    pub(super) fn open_prompt(&mut self, kind: PromptKind, initial: String) -> Result<()> {
        self.prompt = Some(Prompt {
            kind,
            buffer: initial,
        });
        self.draw_status_only()
    }

    pub(super) fn handle_prompt_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<bool> {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let Some(prompt) = self.prompt.as_mut() else {
            return Ok(false);
        };
        match code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Char('c') if ctrl => self.prompt = None,
            // Ctrl+U clears the line, like in a shell.
            KeyCode::Char('u') if ctrl => prompt.buffer.clear(),
            KeyCode::Backspace => {
                prompt.buffer.pop();
            }
            KeyCode::Enter => {
                let Prompt { kind, buffer } = self.prompt.take().unwrap();
                return self.submit_prompt(kind, buffer.trim()).map(|_| false);
            }
            KeyCode::Char(c) => prompt.buffer.push(c),
            _ => return Ok(false),
        }
        self.draw_status_only()?;
        Ok(false)
    }

    fn submit_prompt(&mut self, kind: PromptKind, text: &str) -> Result<()> {
        match kind {
            PromptKind::SaveAs => {
                if text.is_empty() {
                    self.status_msg = Some("save-as cancelled (empty path)".into());
                } else {
                    self.save_to(text);
                }
                self.draw_status_only()
            }
            PromptKind::Export(op) => {
                if text.is_empty() {
                    self.status_msg = Some("export cancelled (empty path)".into());
                    self.draw_status_only()
                } else {
                    self.execute_export(op, text)
                }
            }
            PromptKind::Filter => self.set_filter(text),
            PromptKind::Jump => {
                match text.parse::<usize>() {
                    Ok(n) if n >= 1 && n <= self.files.len() => self.go_to(n - 1),
                    _ => {
                        self.status_msg =
                            Some(format!("enter a number between 1 and {}", self.files.len()));
                    }
                }
                self.redraw_all()
            }
        }
    }

    /// Status bar contents while a prompt is open: (bg, fg, text).
    pub(super) fn prompt_line(&self, prompt: &Prompt) -> (Color, Color, String) {
        let fg = Color::Rgb { r: 230, g: 230, b: 230 };
        let buf = &prompt.buffer;
        match prompt.kind {
            PromptKind::SaveAs => (Color::Rgb { r: 60, g: 80, b: 30 }, fg, format!(" Save as: {buf}_")),
            PromptKind::Export(op) => {
                let verb = match op {
                    ExportOp::Copy => "Copy",
                    ExportOp::Move => "Move",
                };
                let n = self.marked.len();
                (
                    Color::Rgb { r: 30, g: 60, b: 80 },
                    fg,
                    format!(" {verb} {n} marked → dir: {buf}_"),
                )
            }
            PromptKind::Filter => {
                let n = self
                    .all_files
                    .iter()
                    .filter(|p| crate::files::matches_filter(p, buf))
                    .count();
                (
                    Color::Rgb { r: 70, g: 40, b: 90 },
                    fg,
                    format!(" /{buf}_   ({n} match{}, empty = show all)", if n == 1 { "" } else { "es" }),
                )
            }
            PromptKind::Jump => (
                Color::Rgb { r: 70, g: 40, b: 90 },
                fg,
                format!(" Go to image (1–{}): {buf}_", self.files.len()),
            ),
        }
    }
}
