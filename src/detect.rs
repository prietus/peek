use std::env;

use clap::ValueEnum;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Protocol {
    Kitty,
    Iterm,
    Sixel,
    Halfblocks,
}

/// Best-effort detection based on environment. A future version should
/// also probe via DA (Device Attributes) escape sequences.
pub fn detect() -> Protocol {
    if env::var_os("KITTY_WINDOW_ID").is_some() {
        return Protocol::Kitty;
    }

    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();
    let term = env::var("TERM").unwrap_or_default();

    match term_program.as_str() {
        "ghostty" | "WezTerm" => return Protocol::Kitty,
        "iTerm.app" => return Protocol::Iterm,
        _ => {}
    }

    if term.contains("kitty") {
        return Protocol::Kitty;
    }
    if term.contains("foot") || term.contains("mlterm") {
        return Protocol::Sixel;
    }

    Protocol::Halfblocks
}
