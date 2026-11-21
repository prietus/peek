//! Talking to the rest of the desktop: clipboard, "open with", wallpaper and
//! user-defined commands. Paths are always passed to `sh` as positional
//! arguments, never spliced into the script text, so file names with quotes
//! or `$(...)` can't inject shell code.

use std::io::{Cursor, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::DynamicImage;

fn has_program(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn x11() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

/// Run `program args…`, feed `input` on stdin, and wait for it.
fn pipe_to(program: &str, args: &[&str], input: &[u8]) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input)?;
    let status = child.wait()?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

/// Clipboard programs for this session, best first: (program, args for
/// text, args for PNG).
fn clipboard_tools() -> Vec<(&'static str, Vec<&'static str>, Vec<&'static str>)> {
    let mut tools = Vec::new();
    if wayland() && has_program("wl-copy") {
        tools.push(("wl-copy", vec![], vec!["--type", "image/png"]));
    }
    if x11() && has_program("xclip") {
        tools.push((
            "xclip",
            vec!["-selection", "clipboard"],
            vec!["-selection", "clipboard", "-t", "image/png"],
        ));
    }
    if x11() && has_program("xsel") {
        // xsel can't set a MIME type, so it's text-only.
        tools.push(("xsel", vec!["--clipboard", "--input"], vec![]));
    }
    if has_program("pbcopy") {
        tools.push(("pbcopy", vec![], vec![]));
    }
    tools
}

/// Copy text to the clipboard. Falls back to the OSC 52 escape sequence,
/// which most modern terminals honour (also over SSH). Returns how it was
/// copied, for the status bar.
pub fn copy_text(text: &str) -> Result<&'static str> {
    for (program, args, _) in clipboard_tools() {
        if pipe_to(program, &args, text.as_bytes()).is_ok() {
            return Ok(program);
        }
    }
    let mut out = std::io::stdout().lock();
    write!(out, "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
    out.flush()?;
    Ok("terminal (OSC 52)")
}

/// Copy an image to the clipboard as PNG.
pub fn copy_image(img: &DynamicImage) -> Result<&'static str> {
    let mut png = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)?;
    let png = png.into_inner();
    let mut last_err = None;
    for (program, _, args) in clipboard_tools() {
        if args.is_empty() {
            continue;
        }
        match pipe_to(program, &args, &png) {
            Ok(()) => return Ok(program),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no image clipboard tool found (install wl-clipboard or xclip)")))
}

/// Build the `sh -c` script for a user template. Placeholders become
/// positional parameters: {path}/{} → "$1", {dir} → "$2", {name} → "$3".
/// A template without placeholders (or a literal `$1`) gets "$1" appended.
pub fn shell_script(template: &str) -> String {
    let has_placeholder = ["{}", "{path}", "{dir}", "{name}", "$1"]
        .iter()
        .any(|p| template.contains(p));
    let script = template
        .replace("{path}", "\"$1\"")
        .replace("{}", "\"$1\"")
        .replace("{dir}", "\"$2\"")
        .replace("{name}", "\"$3\"");
    if has_placeholder {
        script
    } else {
        format!("{script} \"$1\"")
    }
}

fn shell_command(template: &str, path: &Path) -> Command {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default();
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(shell_script(template))
        .arg("ipeek")
        .arg(path)
        .arg(dir)
        .arg(name);
    cmd
}

/// Start a command in the background, detached from the terminal.
pub fn spawn_background(template: &str, path: &Path) -> Result<()> {
    shell_command(template, path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start `{template}`"))?;
    Ok(())
}

/// Run a command attached to the terminal and wait for it. The caller must
/// have left raw mode / the alternate screen first.
pub fn run_foreground(template: &str, path: &Path) -> Result<std::process::ExitStatus> {
    Ok(shell_command(template, path).status()?)
}

pub fn default_open_command() -> &'static str {
    if cfg!(target_os = "macos") { "open" } else { "xdg-open" }
}

/// Set the desktop wallpaper, using `custom` when configured, otherwise the
/// first method that fits the running desktop. Returns the method used.
pub fn set_wallpaper(path: &Path, custom: Option<&str>) -> Result<String> {
    let path = std::fs::canonicalize(path)?;
    if let Some(cmd) = custom {
        let status = shell_command(cmd, &path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("wallpaper_command exited with {status}");
        }
        return Ok(cmd.to_string());
    }

    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    let run = |template: &str| -> Result<String> {
        let status = shell_command(template, &path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(template.split_whitespace().next().unwrap_or(template).to_string())
        } else {
            Err(anyhow!("`{template}` exited with {status}"))
        }
    };

    if cfg!(target_os = "macos") {
        return run(r#"osascript -e "tell application \"System Events\" to tell every desktop to set picture to \"$1\"""#);
    }
    if desktop.contains("gnome") && has_program("gsettings") {
        return run(
            r#"gsettings set org.gnome.desktop.background picture-uri "file://$1" && gsettings set org.gnome.desktop.background picture-uri-dark "file://$1""#,
        );
    }
    if desktop.contains("kde") && has_program("plasma-apply-wallpaperimage") {
        return run("plasma-apply-wallpaperimage {path}");
    }
    if has_program("swww") {
        return run("swww img {path}");
    }
    if std::env::var_os("SWAYSOCK").is_some() && has_program("swaymsg") {
        return run(r#"swaymsg output '*' bg {path} fill"#);
    }
    if x11() && has_program("feh") {
        return run("feh --bg-fill {path}");
    }
    bail!("don't know how to set the wallpaper here — set wallpaper_command in the config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_become_positional_args() {
        assert_eq!(shell_script("gimp"), r#"gimp "$1""#);
        assert_eq!(shell_script("cp {} {dir}/backup/"), r#"cp "$1" "$2"/backup/"#);
        assert_eq!(shell_script("echo {name} {path}"), r#"echo "$3" "$1""#);
        assert_eq!(shell_script(r#"set "file://$1""#), r#"set "file://$1""#);
    }

    #[test]
    fn hostile_file_names_are_not_executed() {
        let dir = std::env::temp_dir().join(format!("ipeek-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let evil = dir.join("$(touch pwned) ';touch pwned2'.png");
        std::fs::write(&evil, b"x").unwrap();
        let out = dir.join("out.txt");
        let template = format!("printf %s {{name}} > {}", out.display());
        let status = run_foreground(&template, &evil).unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "$(touch pwned) ';touch pwned2'.png"
        );
        assert!(!dir.join("pwned").exists() && !Path::new("pwned").exists());
        assert!(!dir.join("pwned2").exists() && !Path::new("pwned2").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
