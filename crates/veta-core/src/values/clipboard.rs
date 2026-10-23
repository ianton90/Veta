//! Copy and paste as tab-separated text, the format spreadsheets use on the
//! clipboard.

use crate::error::invalid;
use std::ops::Range;

use arrow::array::Array;
use arrow::datatypes::Schema;

use crate::display::PathPart;
use crate::display::{field_array, json_column};
use crate::error::Result;
use crate::model::Document;
use crate::values::node::Node;

/// The cells in `rows` × `columns` as tab-separated text, with the column
/// names first if `header`. Nulls are empty; nested values are JSON.
pub fn copy_text(
    doc: &Document,
    rows: Range<usize>,
    columns: Range<usize>,
    header: bool,
) -> Result<String> {
    let targets: Vec<(usize, Vec<String>)> = columns.map(|c| (c, Vec::new())).collect();
    copy_fields(doc, rows, &targets, header)
}

/// Like [`copy_text`], for columns or fields of struct columns: each target
/// is a column index and the field names down to the field (empty for the
/// column). Field headers are `column.field`.
pub fn copy_fields(
    doc: &Document,
    rows: Range<usize>,
    targets: &[(usize, Vec<String>)],
    header: bool,
) -> Result<String> {
    let mut lines: Vec<Vec<Option<String>>> = Vec::new();
    if header {
        lines.push(header_line(&doc.schema(), targets).collect());
    }
    if !rows.is_empty() {
        let batch = doc.read(rows)?;
        let missing = || invalid!(FieldGone);
        let arrays = targets
            .iter()
            .map(|(c, path)| field_array(batch.column(*c), path).ok_or_else(missing))
            .collect::<Result<Vec<_>>>()?;
        let columns = arrays
            .iter()
            .map(|a| cell_texts(a.as_ref()))
            .collect::<Result<Vec<_>>>()?;
        for r in 0..batch.num_rows() {
            lines.push(columns.iter().map(|c| c[r].clone()).collect());
        }
    }
    Ok(to_tsv(&lines))
}

/// How a column, or a field inside a struct column, is named in headers:
/// `column.field.inner`.
pub fn field_label(column: &str, path: &[impl std::fmt::Display]) -> String {
    let mut label = column.to_owned();
    for part in path {
        label = format!("{label}.{part}");
    }
    label
}

/// Removes the first line of pasted `values` if it holds the names of
/// `targets` (in order, as many as it has), as copied with whole columns.
pub fn drop_header(
    values: &mut Vec<Vec<Option<String>>>,
    schema: &Schema,
    targets: &[(usize, Vec<String>)],
) {
    let Some(first) = values.first() else {
        return;
    };
    let header: Vec<Option<String>> = header_line(schema, targets).take(first.len()).collect();
    if *first == header {
        values.remove(0);
    }
}

fn header_line<'a>(
    schema: &'a Schema,
    targets: &'a [(usize, Vec<String>)],
) -> impl Iterator<Item = Option<String>> + 'a {
    targets
        .iter()
        .map(|(c, path)| Some(field_label(schema.field(*c).name(), path)))
}

/// The value at `path` inside cell (`row`, `column`) as clipboard text:
/// JSON for nested values, empty for null.
pub fn copy_value(doc: &Document, row: usize, column: usize, path: &[PathPart]) -> Result<String> {
    let batch = doc.read(row..row + 1)?;
    let node = Node::new(batch.column(column).clone(), 0)
        .get(path)
        .ok_or_else(|| invalid!(EntryGone))?;
    let text = match node.value() {
        Some(value) => cell_texts(value.as_ref())?.remove(0),
        None => None,
    };
    Ok(to_tsv(&[vec![text]]))
}

/// Each value of `array` as copied: JSON for nested values, `None` for
/// null.
fn cell_texts(array: &dyn Array) -> Result<Vec<Option<String>>> {
    if array.data_type().is_nested() {
        let json = json_column(array)?;
        return Ok((0..json.len())
            .map(|r| json.is_valid(r).then(|| json.value(r).to_owned()))
            .collect());
    }
    let options = arrow::util::display::FormatOptions::default().with_null("null");
    let formatter = arrow::util::display::ArrayFormatter::try_new(array, &options)?;
    Ok((0..array.len())
        .map(|r| array.is_valid(r).then(|| formatter.value(r).to_string()))
        .collect())
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
    fn copies_one_nested_value() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = Document::open(&path, crate::io::OpenOptions::default()).unwrap();
        let column = |name: &str| doc.schema().index_of(name).unwrap();
        let copy =
            |row, name: &str, path: &[PathPart]| copy_value(&doc, row, column(name), path).unwrap();
        assert_eq!(copy(0, "tags", &[PathPart::Index(1)]), "b\n");
        assert_eq!(
            copy(3, "tags", &[PathPart::Index(1)]),
            "\n",
            "null is empty"
        );
        assert_eq!(copy(0, "point", &[PathPart::Field("x".into())]), "1\n");
        let item = parse_tsv(&copy(0, "path", &[PathPart::Index(1)]));
        assert_eq!(
            item,
            [[Some(r#"{"x":3,"y":4}"#.to_owned())]],
            "nested values as JSON"
        );
        assert!(copy_value(&doc, 0, column("tags"), &[PathPart::Index(9)]).is_err());
    }

    #[test]
    fn headers_name_fields_and_are_dropped_on_paste() {
        use arrow::datatypes::{DataType, Field, Fields};
        let point = Fields::from(vec![Field::new("x", DataType::Int64, true)]);
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("point", DataType::Struct(point), true),
        ]);
        assert_eq!(field_label("point", &["x"]), "point.x");
        let targets = vec![(0, Vec::new()), (1, vec!["x".to_owned()])];
        let mut values = parse_tsv("id\tpoint.x\n1\t2\n");
        drop_header(&mut values, &schema, &targets);
        assert_eq!(values, [[Some("1".to_owned()), Some("2".to_owned())]]);
        // Fewer values than targets: compared with the first ones.
        let mut values = parse_tsv("id\n1\n");
        drop_header(&mut values, &schema, &targets);
        assert_eq!(values, [[Some("1".to_owned())]]);
        drop_header(&mut values, &schema, &targets);
        assert_eq!(values.len(), 1, "data stays");
    }

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
