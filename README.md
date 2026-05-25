# ipeek

Terminal image viewer for Kitty, Sixel, iTerm2 and half-block terminals. Browse single
files or whole directories, with grid view, animated GIF/WebP playback, slideshow,
edit operations, and copy/move workflows — all without leaving the terminal.

## Features

- **Auto-detected protocols**: Kitty graphics, Sixel, iTerm2 inline images, half-blocks fallback.
- **Grid view** (default when pointed at a directory) with on-disk thumbnail cache (`$XDG_CACHE_HOME/peek/thumbs/`).
- **Animated GIF and WebP** playback with per-frame delays.
- **Video previews** (mp4 / mkv / webm / mov / avi / m4v / mpeg) when `ffmpeg` is on PATH — thumbnail in grid, short looping animation on open. No audio, no seek.
- **Slideshow** mode (`--slideshow N`, `P` to play/pause).
- **Edit ops**: rotate, flip, crop (`c`), save-as. Rotating/flipping/cropping an
  animation collapses it to the current frame.
- **Mark & export**: in grid, `Space` toggles marks, `A` selects all, `u` clears,
  `y` copies, `M` moves marked files to a destination directory.
- **Background preloader** keeps neighbouring images warm.
- **File-list input**: `-f list.txt` or `-f -` to read paths from stdin (supports `#` comments).
- **Recursive scan** and sort by name / date / size / random.

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
```

## Key bindings

### Single-image view

| Key | Action |
|-----|--------|
| `n` / `→` / `Space` | next image |
| `p` / `←` | previous image |
| `g` / `G` | first / last |
| `r` / `R` | rotate 90° CW / CCW |
| `h` / `v` | flip horizontal / vertical |
| `c` | crop mode (arrows resize, `Enter` apply, `Esc` cancel) |
| `s` | save as… |
| `d` | delete (with confirmation) |
| `Ctrl+R` | reload from disk |
| `P` | toggle slideshow |
| `i` | info overlay |
| `?` | help |
| `m` | menu |
| `q` / `Esc` | quit / back |

### Grid view

| Key | Action |
|-----|--------|
| arrows / `hjkl` | move selection |
| `Enter` | open selected |
| `Space` | toggle mark |
| `A` / `u` | mark all / clear all |
| `y` | copy marked to… |
| `M` | move marked to… |
| `d` | delete current |
| `q` | quit |

Press `?` inside the viewer for the full list.

## Requirements

- A terminal that supports one of: Kitty graphics, Sixel, iTerm2 inline images.
  Half-blocks rendering is used as fallback in plain terminals.
- Rust 1.85+ (edition 2024).
- Optional: `ffmpeg` on PATH to enable video previews. Videos are silently
  skipped when ffmpeg isn't found, so images keep working as before.

## License

[MIT](LICENSE)
