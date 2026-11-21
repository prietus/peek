//! Human-readable EXIF summary for the info panel.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use exif::{Exif, Field, In, Tag, Value};

/// Label/value rows describing the photo's EXIF data, in display order.
/// Empty when the file has no EXIF block (or isn't a format that carries one).
pub fn exif_summary(path: &Path) -> Vec<(String, String)> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(exif) = exif::Reader::new().read_from_container(&mut BufReader::new(file)) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let mut push = |label: &str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            rows.push((label.to_string(), v));
        }
    };

    let make = text(&exif, Tag::Make);
    let model = text(&exif, Tag::Model);
    push("camera", camera_name(make.as_deref(), model.as_deref()));
    push("lens", text(&exif, Tag::LensModel));
    push(
        "taken",
        text(&exif, Tag::DateTimeOriginal)
            .or_else(|| text(&exif, Tag::DateTime))
            .map(|d| pretty_date(&d)),
    );
    push("exposure", shown(&exif, Tag::ExposureTime));
    push("aperture", shown(&exif, Tag::FNumber));
    push(
        "ISO",
        shown(&exif, Tag::PhotographicSensitivity),
    );
    push("focal", shown(&exif, Tag::FocalLength));
    push("GPS", gps(&exif));
    rows
}

fn field(exif: &Exif, tag: Tag) -> Option<&Field> {
    exif.get_field(tag, In::PRIMARY)
}

/// ASCII tag value without the quotes `display_value` adds.
fn text(exif: &Exif, tag: Tag) -> Option<String> {
    match &field(exif, tag)?.value {
        Value::Ascii(parts) => {
            let s = parts
                .iter()
                .map(|p| String::from_utf8_lossy(p).trim().to_string())
                .collect::<Vec<_>>()
                .join(" ");
            Some(s.trim_end_matches('\0').trim().to_string())
        }
        _ => None,
    }
}

/// Formatted value with units, e.g. "1/250 s", "f/2.8", "35 mm".
fn shown(exif: &Exif, tag: Tag) -> Option<String> {
    let f = field(exif, tag)?;
    Some(f.display_value().with_unit(exif).to_string())
}

/// Avoid "Canon Canon EOS R6": many cameras repeat the make in the model.
fn camera_name(make: Option<&str>, model: Option<&str>) -> Option<String> {
    match (make, model) {
        (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.to_lowercase()) => {
            Some(model.to_string())
        }
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (Some(one), None) | (None, Some(one)) => Some(one.to_string()),
        (None, None) => None,
    }
}

fn gps(exif: &Exif) -> Option<String> {
    let coord = |tag: Tag, ref_tag: Tag| -> Option<f64> {
        let Value::Rational(parts) = &field(exif, tag)?.value else {
            return None;
        };
        let dms: Vec<f64> = parts.iter().map(|r| r.to_f64()).collect();
        let negative = text(exif, ref_tag).is_some_and(|r| r == "S" || r == "W");
        dms_to_decimal(&dms, negative)
    };
    let lat = coord(Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let lon = coord(Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    Some(format!("{lat:.5}, {lon:.5}"))
}

/// EXIF writes dates as "2026:07:14 18:30:00"; show "2026-07-14 18:30:00".
fn pretty_date(raw: &str) -> String {
    match raw.split_once(' ') {
        Some((date, time)) if date.len() == 10 => format!("{} {time}", date.replace(':', "-")),
        _ => raw.to_string(),
    }
}

/// Degrees/minutes/seconds (any trailing parts optional) to signed decimal degrees.
fn dms_to_decimal(dms: &[f64], negative: bool) -> Option<f64> {
    let deg = *dms.first()?;
    let min = dms.get(1).copied().unwrap_or(0.0);
    let sec = dms.get(2).copied().unwrap_or(0.0);
    let v = deg + min / 60.0 + sec / 3600.0;
    v.is_finite().then_some(if negative { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dms_conversion() {
        let v = dms_to_decimal(&[40.0, 25.0, 1.2], false).unwrap();
        assert!((v - 40.417).abs() < 1e-3);
        assert!(dms_to_decimal(&[3.0, 30.0], true).unwrap() == -3.5);
        assert_eq!(dms_to_decimal(&[], false), None);
    }

    #[test]
    fn exif_dates_use_dashes() {
        assert_eq!(pretty_date("2026:07:14 18:30:00"), "2026-07-14 18:30:00");
        assert_eq!(pretty_date("garbage"), "garbage");
    }

    #[test]
    fn camera_name_dedupes_make() {
        assert_eq!(camera_name(Some("Canon"), Some("Canon EOS R6")).unwrap(), "Canon EOS R6");
        assert_eq!(camera_name(Some("SONY"), Some("ILCE-7M3")).unwrap(), "SONY ILCE-7M3");
        assert_eq!(camera_name(None, None), None);
    }

    #[test]
    fn non_exif_file_yields_nothing() {
        assert!(exif_summary(Path::new("/nonexistent.jpg")).is_empty());
    }
}
