use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum SortMode {
    Name,
    Date,
    Size,
    Random,
}

/// Result of resolving the CLI args into a flat list of image paths.
pub struct Collection {
    pub files: Vec<PathBuf>,
    pub start_index: usize,
    /// True when the user pointed peek at a directory (so we should open grid).
    pub started_from_directory: bool,
}

pub fn collect(
    path: &Path,
    from_file: Option<&str>,
    recursive: bool,
    sort: SortMode,
) -> Result<Collection> {
    if let Some(ff) = from_file {
        return collection_from_list(ff);
    }
    if path.as_os_str() == "-" {
        return collection_from_list("-");
    }

    let (dir, start) = if path.is_dir() {
        (path.to_path_buf(), None)
    } else {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        (parent, Some(path.to_path_buf()))
    };

    let mut files = Vec::new();
    scan_dir(&dir, recursive, &mut files)?;
    sort_files(&mut files, sort);

    if files.is_empty() {
        return Err(anyhow!("no images in {}", dir.display()));
    }

    let start_index = start
        .as_ref()
        .and_then(|p| files.iter().position(|f| f == p))
        .unwrap_or(0);

    Ok(Collection {
        files,
        start_index,
        started_from_directory: start.is_none(),
    })
}

fn collection_from_list(source: &str) -> Result<Collection> {
    let files = read_filelist(source)?;
    if files.is_empty() {
        return Err(anyhow!("no usable image paths in {}", source));
    }
    Ok(Collection {
        files,
        start_index: 0,
        started_from_directory: true,
    })
}

fn scan_dir(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                scan_dir(&path, true, out)?;
            }
        } else if has_media_ext(&path) {
            out.push(path);
        }
    }
    Ok(())
}

pub fn has_image_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map(|e| IMAGE_EXTS.contains(&e.as_str()))
        .unwrap_or(false)
}

/// True for images, plus videos when ffmpeg is installed. Videos are filtered
/// out when ffmpeg is missing so the user doesn't see broken thumbs.
pub fn has_media_ext(path: &Path) -> bool {
    if has_image_ext(path) {
        return true;
    }
    crate::video::is_video(path) && crate::video::ffmpeg_available()
}

fn sort_files(files: &mut [PathBuf], mode: SortMode) {
    match mode {
        SortMode::Name => files.sort(),
        SortMode::Date => {
            files.sort_by_cached_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        }
        SortMode::Size => {
            files.sort_by_cached_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0));
        }
        SortMode::Random => {
            let seed = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0xCAFE_BABE_DEAD_BEEF);
            shuffle(files, seed);
        }
    }
}

/// Fisher–Yates with an inline xorshift64. Avoids an extra `rand` dep.
fn shuffle<T>(v: &mut [T], mut seed: u64) {
    if v.len() < 2 {
        return;
    }
    if seed == 0 {
        seed = 0x9E37_79B9_7F4A_7C15;
    }
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

fn read_filelist(source: &str) -> Result<Vec<PathBuf>> {
    let raw: Vec<String> = if source == "-" {
        io::stdin()
            .lock()
            .lines()
            .collect::<io::Result<Vec<_>>>()
            .context("failed to read paths from stdin")?
    } else {
        let f =
            fs::File::open(source).with_context(|| format!("failed to open list {}", source))?;
        io::BufReader::new(f)
            .lines()
            .collect::<io::Result<Vec<_>>>()
            .with_context(|| format!("failed to read list {}", source))?
    };

    Ok(raw
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect())
}
