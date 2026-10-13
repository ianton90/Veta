//! Copy and paste as tab-separated text, the format spreadsheets use on the
//! clipboard.

use std::ops::Range;

use arrow::array::Array;

use crate::display::{format_batch, json_column};
use crate::error::{Error, Result};
use crate::model::Document;

/// Most cells copied at once, to keep the clipboard (and memory) sane.
pub const MAX_COPY_CELLS: usize = 1_000_000;

/// The cells in `rows` × `columns` as tab-separated text, with the column
/// names first if `header`. Nulls are empty; nested values are JSON.
pub fn copy_text(
    doc: &Document,
    rows: Range<usize>,
    columns: Range<usize>,
    header: bool,
) -> Result<String> {
    let cells = rows.len().saturating_mul(columns.len());
    if cells > MAX_COPY_CELLS {
        return Err(Error::InvalidCommand(format!(
            "that's {cells} cells; at most {MAX_COPY_CELLS} can be copied at once"
        )));
    }
    let schema = doc.schema();
    let mut lines: Vec<Vec<Option<String>>> = Vec::new();
    if header {
        lines.push(
            columns
                .clone()
                .map(|c| Some(schema.field(c).name().clone()))
                .collect(),
        );
    }
    if !rows.is_empty() {
        let batch = doc.read(rows)?;
        let text = format_batch(&batch)?;
        let mut json = Vec::new();
        for c in columns.clone() {
            json.push(
                batch
                    .column(c)
                    .data_type()
                    .is_nested()
                    .then(|| json_column(batch.column(c).as_ref()))
                    .transpose()?,
            );
        }
        for (r, row) in text.into_iter().enumerate() {
            lines.push(
                columns
                    .clone()
                    .zip(&json)
                    .map(|(c, json)| match json {
                        Some(json) => json.is_valid(r).then(|| json.value(r).to_owned()),
                        None => row[c].clone(),
                    })
                    .collect(),
            );
        }
    }
    Ok(to_tsv(&lines))
}

/// Tab-separated text; values with tabs, line breaks or quotes are quoted.
pub fn to_tsv(lines: &[Vec<Option<String>>]) -> String {
    let mut out = String::new();
    for line in lines {
        let cells: Vec<String> = line
            .iter()
            .map(|v| {
                let v = v.as_deref().unwrap_or("");
                if v.contains(['\t', '\n', '\r', '"']) {
                    format!("\"{}\"", v.replace('"', "\"\""))
                } else {
                    v.to_owned()
                }
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    out
}

/// Parses tab-separated text (as copied from Veta or a spreadsheet). Empty
/// cells are `None`. A final line break doesn't add a row.
pub fn parse_tsv(text: &str) -> Vec<Vec<Option<String>>> {
    let mut lines = Vec::new();
    let mut line: Vec<Option<String>> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let finish = |cell: &mut String, line: &mut Vec<Option<String>>| {
        let value = std::mem::take(cell);
        line.push((!value.is_empty()).then_some(value));
    };
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            '"' if quoted => quoted = false,
            '"' if cell.is_empty() => quoted = true,
            '\t' if !quoted => finish(&mut cell, &mut line),
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' | '\r' if !quoted => {
                finish(&mut cell, &mut line);
                lines.push(std::mem::take(&mut line));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !line.is_empty() {
        finish(&mut cell, &mut line);
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_round_trip() {
        let lines = vec![
            vec![Some("a".into()), None, Some("tab\there".into())],
            vec![
                Some("say \"hi\"".into()),
                Some("two\nlines".into()),
                Some("x".into()),
            ],
        ];
        let text = to_tsv(&lines);
        assert_eq!(parse_tsv(&text), lines);
        // Spreadsheets end lines with CRLF.
        assert_eq!(
            parse_tsv("1\t2\r\n3\t\r\n"),
            vec![
                vec![Some("1".into()), Some("2".into())],
                vec![Some("3".into()), None]
            ]
        );
        assert_eq!(parse_tsv("single"), vec![vec![Some("single".into())]]);
    }

    #[test]
    fn copies_cells_with_nested_values_as_json() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = Document::open(&path, crate::OpenOptions::default()).unwrap();
        let text = copy_text(&doc, 0..2, 0..3, true).unwrap();
        assert_eq!(
            text,
            "id\tpoint\ttags\n0\t\"{\"\"x\"\":1,\"\"y\"\":2}\"\t\"[\"\"a\"\",\"\"b\"\"]\"\n1\t\"{\"\"x\"\":-1,\"\"y\"\":null}\"\t[]\n"
        );
    }
}
