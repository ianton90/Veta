//! Plain-text table output.

use std::io::{self, Write};

/// Longest cell content printed, in characters; longer values are cut with `…`.
const MAX_CELL: usize = 40;
const NULL: &str = "null";

/// Writes a table with a header row, an optional second header row (types),
/// a separator and the data rows. `right[i]` right-aligns column `i`.
pub fn write(
    out: &mut impl Write,
    header: &[String],
    subheader: Option<&[String]>,
    rows: &[Vec<Option<String>>],
    right: &[bool],
) -> io::Result<()> {
    let cell = |value: &Option<String>| truncate(value.as_deref().unwrap_or(NULL));

    let mut widths: Vec<usize> = header.iter().map(|h| truncate(h).chars().count()).collect();
    if let Some(sub) = subheader {
        for (w, s) in widths.iter_mut().zip(sub) {
            *w = (*w).max(truncate(s).chars().count());
        }
    }
    for row in rows {
        for (w, value) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell(value).chars().count());
        }
    }

    let line = |out: &mut dyn Write, values: &mut dyn Iterator<Item = String>| -> io::Result<()> {
        let parts: Vec<String> = values
            .zip(&widths)
            .zip(right)
            .map(|((v, &w), &r)| {
                if r {
                    format!("{v:>w$}")
                } else {
                    format!("{v:<w$}")
                }
            })
            .collect();
        writeln!(out, "{}", parts.join(" │ ").trim_end())
    };

    line(out, &mut header.iter().map(|h| truncate(h)))?;
    if let Some(sub) = subheader {
        line(out, &mut sub.iter().map(|s| truncate(s)))?;
    }
    let separator: Vec<String> = widths.iter().map(|&w| "─".repeat(w)).collect();
    writeln!(out, "{}", separator.join("─┼─"))?;
    for row in rows {
        line(out, &mut row.iter().map(cell))?;
    }
    Ok(())
}

fn truncate(s: &str) -> String {
    // Control characters (newlines, tabs) would break the layout.
    let clean: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() <= MAX_CELL {
        clean
    } else {
        let mut cut: String = clean.chars().take(MAX_CELL - 1).collect();
        cut.push('…');
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_and_truncates() {
        let mut out = Vec::new();
        let long = "x".repeat(50);
        write(
            &mut out,
            &["a".into(), "num".into()],
            None,
            &[
                vec![Some(long), Some("1".into())],
                vec![None, Some("100".into())],
            ],
            &[false, true],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], format!("a{} │ num", " ".repeat(MAX_CELL - 1)));
        assert!(lines[2].starts_with(&"x".repeat(MAX_CELL - 1)));
        assert!(lines[2].ends_with("… │   1"));
        assert!(lines[3].starts_with("null"));
        assert!(lines[3].ends_with("│ 100"));
    }
}
