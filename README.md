# ipeek

Terminal image viewer for Kitty, Sixel, iTerm2 and half-block terminals. Browse single
files or whole directories, with grid view, animated GIF/WebP playback, slideshow,
edit operations, and copy/move workflows — all without leaving the terminal.

## Features

- **Auto-detected protocols**: Kitty graphics, Sixel, iTerm2 inline images, half-blocks fallback.
- **Grid view** (default when pointed at a directory) with on-disk thumbnail cache (`$XDG_CACHE_HOME/ipeek/thumbs/`).
- **Formats**: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, TGA, PNM, QOI and HDR built in;
  AVIF, HEIC/HEIF and JPEG XL when ImageMagick (`magick`) or `ffmpeg` is installed.
- **EXIF aware**: photos are shown upright (EXIF orientation), and the info panel (`i`)
  shows camera, lens, date taken, exposure, aperture, ISO, focal length and GPS.
- **Animated GIF and WebP** playback with per-frame delays.
- **Video previews** (mp4 / mkv / webm / mov / avi / m4v / mpeg) when `ffmpeg` is on PATH — thumbnail in grid, short looping animation on open. No audio, no seek.
- **Slideshow** mode (`--slideshow N`, `P` to play/pause).
- **Edit ops**: rotate, flip, crop (`c`), brightness, contrast, grayscale, invert,
  blur, sharpen and resize (`z`), with a 10-step undo (`u`). Editing an animation
  collapses it to the current frame (undo brings the animation back). Saving as
  JPEG converts transparent images automatically.
- **Filter & jump**: `/` narrows the listing by file name, `:` jumps to image N,
  `O` cycles the sort order without restarting.
- **Desktop integration**: open in another program (`o`), copy the image or its path
  to the clipboard (`y` / `Y`), set it as wallpaper (`W`), and bind your own shell
  commands to keys from the config file.
- **Mark & export**: in grid, `Space` toggles marks, `A` selects all, `u` clears,
  `y` copies, `M` moves marked files to a destination directory.
- **Background preloader** keeps neighbouring images warm.
- **File-list input**: `-f list.txt` or `-f -` to read paths from stdin (supports `#` comments).
- **Recursive scan** and sort by name / date / size / random.
- **Config file** (`~/.config/ipeek/config`) for defaults and custom commands.

## Install

```sh
cargo install --path .
```

Or build a release binary:

```sh
cargo build --release
# binary at target/release/ipeek
```

## Usage

```sh
ipeek                       # browse current directory in grid view
ipeek photo.png             # open a single image
ipeek ~/Pictures            # grid view of a directory
ipeek -r ~/Pictures         # recurse into subdirectories
ipeek --sort date ~/shots   # sort by mtime
ipeek --slideshow 3 album/  # auto-advance every 3 seconds
ipeek -f playlist.txt       # read paths from a file
ls *.jpg | ipeek -f -       # read paths from stdin
ipeek --one-shot -1 a.png   # render once and exit (no TUI)
ipeek --protocol sixel x.gif
ipeek --print-config > ~/.config/ipeek/config   # start a config file
```

## Key bindings

### Single-image view

| Key | Action |
|-----|--------|
| `n` / `Space` | next image |
| `p` | previous image |
| `j` `k` / `↓` `↑` / `→` `←` | next / previous (pan when zoomed) |
| `h` / `l` | pan left / right when zoomed |
| `g` / `G` | first / last |
| `+` / `-` / `0` | zoom in / out / reset |
| `r` / `R` | rotate 90° CW / CCW |
| `f` / `F` | flip horizontal / vertical |
| `b` / `B` | brightness up / down |
| `e` / `E` | contrast up / down |
| `z` | resize (`50%`, `800`, `x600`, `800x600` fits, `800x600!` exact) |
| `u` | undo last edit |
| `m` | edit menu (also grayscale, invert, blur, sharpen) |
| `c` | crop mode (`hjkl` move, `HJKL` resize, `a` all, `Enter` apply, `Esc` cancel) |
| `s` / `S` | save / save as… |
| `d` | delete (with confirmation) |
| `Ctrl+R` | reload from disk |
| `P` | toggle slideshow |
| `t` / `Tab` | toggle grid |
| `y` / `Y` | copy image (with edits) / copy path to clipboard |
| `i` | info overlay (with EXIF) |
| `?` | help |
| `q` / `Esc` | quit / back |

### Grid view

| Key | Action |
|-----|--------|
| arrows / `hjkl` | move selection |
| `Enter` | open selected |
| `g` / `G` | first / last |
| `i` | info about selected |
| `t` / `Tab` / `Esc` | back to single view |
| `Space` | toggle mark |
| `A` / `u` | mark all / clear all |
| `y` | copy marked to… |
| `M` | move marked to… |
| `Y` | copy path to clipboard |
| `d` | delete current |
| `q` | quit |

### Both views

| Key | Action |
|-----|--------|
| `/` | filter by name (terms are ANDed; a term with `/` matches the path; empty clears) |
| `:` | go to image number |
| `O` | cycle sort: name → date → size → random |
| `o` | open with external program |
| `W` | set as wallpaper |

Text prompts accept `Ctrl+U` to clear the line and `Esc` to cancel.
Press `?` inside the viewer for the full list.

## Configuration

ipeek reads `$XDG_CONFIG_HOME/ipeek/config` (usually `~/.config/ipeek/config`).
Use `--config FILE` to read another file, `--no-config` to skip it, and
`ipeek --print-config` for a commented example. Command-line flags win over the file.
Problems in the file are shown in the status bar at startup.

```ini
protocol = sixel             # kitty, iterm, sixel, halfblocks
sort = date                  # name, date, size, random
recursive = true
slideshow_interval = 4       # seconds, used by P
open_command = gimp          # used by o (default: xdg-open / open)
wallpaper_command = swww img # used by W (default: GNOME, KDE, swww, sway or feh)

# Custom commands: cmd.<key> = shell command
# Placeholders: {path} (or {}), {dir}, {name}; without one the path is appended.
# Prefix with ! to run in the terminal and wait (e.g. pagers, editors).
cmd.x = !exiftool {path} | less
cmd.X = gimp {path}
```

Custom commands take precedence over built-in keys, and work in both views.
File names are passed to the shell as arguments, so unusual names are safe.

## Clipboard

`y` / `Y` use `wl-copy` on Wayland, `xclip` or `xsel` on X11 and `pbcopy` on macOS.
Copying a path falls back to the OSC 52 terminal escape when none is installed;
copying an image needs `wl-copy` or `xclip`.

## Requirements

- A terminal that supports one of: Kitty graphics, Sixel, iTerm2 inline images.
  Half-blocks rendering is used as fallback in plain terminals.
- Rust 1.85+ (edition 2024).
- Optional: `ffmpeg` on PATH to enable video previews. Videos are silently
  skipped when ffmpeg isn't found, so images keep working as before.
- Optional: ImageMagick 7 (`magick`) or `ffmpeg` for AVIF, HEIC/HEIF and JPEG XL.
- Optional: `wl-clipboard`, `xclip` or `xsel` for the clipboard keys.

## License

[MIT](LICENSE)
