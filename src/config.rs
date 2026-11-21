//! User configuration: `$XDG_CONFIG_HOME/ipeek/config` (or
//! `~/.config/ipeek/config`), a plain `key = value` file. Command-line flags
//! override anything set here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::detect::Protocol;
use crate::files::SortMode;

pub const SAMPLE: &str = r#"# ipeek configuration — copy to ~/.config/ipeek/config
# Lines are `key = value`; `#` starts a comment. Command-line flags win.

# Graphics protocol: kitty, iterm, sixel or halfblocks (default: auto-detect)
# protocol = sixel

# Initial sort order: name, date, size or random
# sort = date

# Always recurse into subdirectories
# recursive = true

# Seconds between images when the slideshow (P) is playing
# slideshow_interval = 4

# Program used by `o` to open the current image (default: xdg-open / open)
# open_command = gimp

# Command used by `W` to set the wallpaper (default: auto-detect GNOME, KDE,
# swww, sway or feh)
# wallpaper_command = swww img

# Custom commands bound to single keys, available in the viewer and the grid.
# Placeholders: {path} (also {}), {dir}, {name}. Without a placeholder the
# path is appended. They run in the background; start the command with `!`
# to run it in the terminal and wait for it (the viewer comes back after).
# cmd.x = !exiftool {path} | less
# cmd.X = gimp {path}
# cmd.T = mv {path} ~/.local/share/Trash/files/
"#;

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub protocol: Option<Protocol>,
    pub sort: Option<SortMode>,
    pub recursive: bool,
    pub slideshow_interval: Option<f32>,
    pub open_command: Option<String>,
    pub wallpaper_command: Option<String>,
    /// Key → shell command template.
    pub commands: BTreeMap<char, String>,
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("ipeek").join("config"))
}

/// Load the config from `path`, or from the default location when `path` is
/// None. A missing default file is not an error; a missing explicit one is.
/// Returns the config plus warnings about lines that were ignored.
pub fn load(path: Option<&Path>) -> Result<(Config, Vec<String>)> {
    let (path, explicit) = match path {
        Some(p) => (p.to_path_buf(), true),
        None => match default_path() {
            Some(p) => (p, false),
            None => return Ok((Config::default(), Vec::new())),
        },
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if !explicit && e.kind() == std::io::ErrorKind::NotFound => {
            Ok((Config::default(), Vec::new()))
        }
        Err(e) => Err(e).with_context(|| format!("failed to read config {}", path.display())),
    }
}

pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warnings = Vec::new();

    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lineno = n + 1;
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("config line {lineno}: expected `key = value`"));
            continue;
        };
        let key = key.trim();
        let value = unquote(value.trim());
        let mut bad = |what: &str| warnings.push(format!("config line {lineno}: {what}"));

        match key {
            "protocol" => match Protocol::from_str(value, true) {
                Ok(p) => cfg.protocol = Some(p),
                Err(_) => bad("protocol must be kitty, iterm, sixel or halfblocks"),
            },
            "sort" => match SortMode::parse(value) {
                Some(s) => cfg.sort = Some(s),
                None => bad("sort must be name, date, size or random"),
            },
            "recursive" => match parse_bool(value) {
                Some(b) => cfg.recursive = b,
                None => bad("recursive must be true or false"),
            },
            "slideshow_interval" => match value.parse::<f32>() {
                Ok(s) if s > 0.0 && s.is_finite() => cfg.slideshow_interval = Some(s),
                _ => bad("slideshow_interval must be a positive number of seconds"),
            },
            "open_command" => cfg.open_command = non_empty(value),
            "wallpaper_command" => cfg.wallpaper_command = non_empty(value),
            _ => {
                if let Some(k) = key.strip_prefix("cmd.") {
                    let mut chars = k.chars();
                    match (chars.next(), chars.next(), non_empty(value)) {
                        (Some(c), None, Some(cmd)) if !c.is_whitespace() => {
                            cfg.commands.insert(c, cmd);
                        }
                        (_, _, None) => bad("command is empty"),
                        _ => bad("cmd.<key> needs exactly one character, e.g. cmd.x"),
                    }
                } else {
                    bad(&format!("unknown key `{key}`"));
                }
            }
        }
    }
    (cfg, warnings)
}

fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_keys() {
        let (cfg, warnings) = parse(
            r#"
            # comment
            protocol = Sixel
            sort = "date"
            recursive = yes
            slideshow_interval = 2.5
            open_command = gimp
            wallpaper_command = 'swww img'
            cmd.x = !exiftool {path} | less
            cmd.T = mv {} ~/trash/
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.protocol, Some(Protocol::Sixel));
        assert_eq!(cfg.sort, Some(SortMode::Date));
        assert!(cfg.recursive);
        assert_eq!(cfg.slideshow_interval, Some(2.5));
        assert_eq!(cfg.open_command.as_deref(), Some("gimp"));
        assert_eq!(cfg.wallpaper_command.as_deref(), Some("swww img"));
        assert_eq!(cfg.commands[&'x'], "!exiftool {path} | less");
        assert_eq!(cfg.commands[&'T'], "mv {} ~/trash/");
    }

    #[test]
    fn bad_lines_become_warnings_not_errors() {
        let (cfg, warnings) = parse(
            "protocol = vga\nnonsense\nfoo = 1\nslideshow_interval = -1\ncmd.ab = x\ncmd.y =\n",
        );
        assert_eq!(warnings.len(), 6, "{warnings:?}");
        assert!(warnings[0].contains("line 1"));
        assert_eq!(cfg.protocol, None);
        assert!(cfg.commands.is_empty());
    }

    #[test]
    fn sample_config_is_valid() {
        let uncommented: String = SAMPLE
            .lines()
            .map(|l| {
                l.strip_prefix("# ")
                    .filter(|l| {
                        l.split_once(" = ").is_some_and(|(k, _)| {
                            k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                        })
                    })
                    .unwrap_or("")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let (cfg, warnings) = parse(&uncommented);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.commands.len(), 3);
    }

    #[test]
    fn missing_default_file_is_fine_but_explicit_is_not() {
        assert!(load(Some(Path::new("/nonexistent/ipeek.conf"))).is_err());
    }
}
