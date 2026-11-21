use std::path::PathBuf;

pub fn resolve_dest_dir(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    if trimmed == "~"
        && let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    PathBuf::from(trimmed)
}

pub fn human_bytes(n: u64) -> String {
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

pub fn truncate_middle(s: &str, max: usize) -> String {
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

pub fn pad_or_truncate(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count >= width {
        s.chars().take(width).collect()
    } else {
        let mut out = String::from(s);
        out.extend(std::iter::repeat_n(' ', width - count));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536 * 1024), "1.5 MB");
    }

    #[test]
    fn truncate_middle_keeps_ends() {
        assert_eq!(truncate_middle("short", 10), "short");
        let t = truncate_middle("/very/long/path/to/image.png", 12);
        assert_eq!(t.chars().count(), 12);
        assert!(t.starts_with("/very") && t.ends_with(".png") && t.contains('…'));
    }

    #[test]
    fn pad_or_truncate_exact_width() {
        assert_eq!(pad_or_truncate("ab", 4), "ab  ");
        assert_eq!(pad_or_truncate("abcdef", 3), "abc");
        assert_eq!(pad_or_truncate("ñé", 3).chars().count(), 3);
    }

    #[test]
    fn resolve_dest_dir_expands_tilde() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(resolve_dest_dir("~"), PathBuf::from(&home));
        assert_eq!(resolve_dest_dir(" ~/x "), PathBuf::from(&home).join("x"));
        assert_eq!(resolve_dest_dir("/tmp/a"), PathBuf::from("/tmp/a"));
    }
}
