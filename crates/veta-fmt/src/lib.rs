//! Display formatting shared by Veta's front ends (the GUI and the CLI):
//! sizes, shortened values, statistics and alignment. `veta-core` holds the
//! model and its data; how it reads on screen lives here or in each front
//! end (DECISIONS 53, 54).

use veta_core::arrow::datatypes::DataType;
use veta_core::stats::ColumnStats;

/// Whether values of this type read best right-aligned.
pub fn is_numeric(data_type: &DataType) -> bool {
    data_type.is_numeric() || matches!(data_type, DataType::Dictionary(_, v) if v.is_numeric())
}

/// Shortens `value` to at most `max` characters for display, noting how much
/// was cut, e.g. `abcdef… (1.2 KiB)`.
pub fn abbreviate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let head: String = value.chars().take(max).collect();
    format!("{head}… ({})", human_bytes(value.len() as u64))
}

/// Formats a byte count with binary units, e.g. `1.5 MiB`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A mean or deviation for display: up to 4 decimals, trailing zeros
/// trimmed; scientific notation for very large or small values.
pub fn format_float(x: f64) -> String {
    let magnitude = x.abs();
    if magnitude != 0.0 && !(1e-4..1e15).contains(&magnitude) {
        return format!("{x:.4e}");
    }
    let text = format!("{x:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// The distinct count for display (`~` when estimated).
pub fn format_distinct(stats: &ColumnStats) -> Option<String> {
    let n = stats.distinct?;
    Some(if stats.distinct_estimated {
        format!("~{n}")
    } else {
        n.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviates_long_values() {
        assert_eq!(abbreviate("short", 10), "short");
        assert_eq!(abbreviate(&"x".repeat(2048), 3), "xxx… (2.0 KiB)");
    }

    #[test]
    fn byte_sizes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn floats() {
        assert_eq!(format_float(99_999.5), "99999.5");
        assert_eq!(format_float(1.0 / 3.0), "0.3333");
        assert_eq!(format_float(2.0), "2");
        assert_eq!(format_float(-0.00001), "-1.0000e-5");
        assert_eq!(format_float(0.0), "0");
    }

    #[test]
    fn numeric_types_align_right() {
        assert!(is_numeric(&DataType::Float64));
        assert!(!is_numeric(&DataType::Utf8));
    }
}
