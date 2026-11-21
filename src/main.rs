mod cache;
mod config;
mod decode;
mod metadata;
mod detect;
mod external;
mod files;
mod grid;
mod render;
mod video;
mod util;
mod viewer;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use image::DynamicImage;

use crate::detect::Protocol;
use crate::files::SortMode;
use crate::viewer::Viewer;

#[derive(Parser, Debug)]
#[command(name = "ipeek", version, about = "Terminal image viewer (Kitty/Sixel/iTerm2/half-blocks)")]
struct Args {
    /// Image file, directory, or "-" to read newline-separated paths from stdin.
    /// Defaults to current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Force a specific protocol
    #[arg(long, value_enum)]
    protocol: Option<Protocol>,

    /// Render once and exit (no TUI). Requires PATH to be a file.
    #[arg(long, short = '1')]
    one_shot: bool,

    /// Recurse into subdirectories when PATH is a directory
    #[arg(long, short = 'r')]
    recursive: bool,

    /// Sort order for the file list [default: name]
    #[arg(long, value_enum)]
    sort: Option<SortMode>,

    /// Read newline-separated image paths from this file (use "-" for stdin)
    #[arg(long = "from-file", short = 'f')]
    from_file: Option<String>,

    /// Auto-advance every N seconds (P toggles play/pause inside the viewer)
    #[arg(long, value_name = "SECONDS")]
    slideshow: Option<f32>,

    /// Read settings from this file instead of ~/.config/ipeek/config
    #[arg(long, value_name = "FILE", conflicts_with = "no_config")]
    config: Option<PathBuf>,

    /// Ignore the config file
    #[arg(long)]
    no_config: bool,

    /// Print a commented example config file and exit
    #[arg(long)]
    print_config: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.print_config {
        print!("{}", config::SAMPLE);
        return Ok(());
    }
    let (cfg, warnings) = if args.no_config {
        Default::default()
    } else {
        config::load(args.config.as_deref())?
    };
    let protocol = args
        .protocol
        .or(cfg.protocol)
        .unwrap_or_else(detect::detect);
    let sort = args.sort.or(cfg.sort).unwrap_or(SortMode::Name);
    let renderer = render::for_protocol(protocol);

    if args.one_shot {
        if !args.path.is_file() {
            anyhow::bail!("--one-shot requires a file path, got {}", args.path.display());
        }
        let img: DynamicImage = decode::open(&args.path)
            .with_context(|| format!("failed to open image: {}", args.path.display()))?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        renderer.render(&img, cols, rows)?;
        return Ok(());
    }

    let collection = files::collect(
        &args.path,
        args.from_file.as_deref(),
        args.recursive || cfg.recursive,
        sort,
    )?;
    let mut viewer = Viewer::from_collection(collection, renderer, sort, cfg, warnings)?;
    if let Some(secs) = args.slideshow {
        viewer.start_slideshow(secs);
    }
    viewer.run()
}
