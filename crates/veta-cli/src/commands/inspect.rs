//! Reading files: `info`, `schema`, `head` and `stats`.

use std::io::Write;

use veta_core::Document;
use veta_core::arrow::datatypes::{DataType, Field};
use veta_core::display::{format_batch, type_name};
use veta_fmt::{abbreviate, human_bytes};

use crate::{Error, table};

pub(crate) fn info(doc: &Document, out: &mut impl Write) -> Result<(), Error> {
    let writer = doc.writer_settings();
    let meta = doc.metadata();
    if let Some(path) = doc.path() {
        writeln!(out, "File:        {}", path.display())?;
    }
    if let Some(info) = doc.file_info() {
        writeln!(
            out,
            "Size:        {} ({} uncompressed)",
            human_bytes(info.file_size),
            human_bytes(info.uncompressed_size())
        )?;
        writeln!(out, "Rows:        {}", doc.num_rows())?;
        writeln!(out, "Columns:     {}", doc.num_columns())?;
        writeln!(
            out,
            "Row groups:  {} (max {} rows)",
            info.row_groups.len(),
            writer.max_row_group_rows
        )?;
    }
    writeln!(out, "Format:      {}", writer.format_version)?;
    if let Some(created_by) = &meta.created_by {
        writeln!(out, "Created by:  {created_by}")?;
    }

    if !meta.key_value.is_empty() {
        writeln!(out, "\nMetadata:")?;
        for kv in &meta.key_value {
            match &kv.value {
                Some(value) => writeln!(out, "  {} = {}", kv.key, abbreviate(value, 80))?,
                None => writeln!(out, "  {}", kv.key)?,
            }
        }
    }

    writeln!(out)?;
    print_writer_settings(doc, out)
}

/// Prints per-column writer settings as a table.
pub(crate) fn print_writer_settings(doc: &Document, out: &mut impl Write) -> Result<(), Error> {
    let writer = doc.writer_settings();
    writeln!(
        out,
        "Writer: format {}, {} rows per row group",
        writer.format_version, writer.max_row_group_rows
    )?;
    let schema = doc.schema();
    let header = [
        "column",
        "compression",
        "encoding",
        "dictionary",
        "statistics",
        "bloom",
    ];
    let mut paths: Vec<String> = writer.columns.iter().map(|(p, _)| p.clone()).collect();
    for f in schema.fields() {
        if !paths.contains(f.name()) {
            paths.push(f.name().clone());
        }
    }
    let rows = paths
        .iter()
        .map(|path| {
            let c = writer.column(path);
            vec![
                Some(path.clone()),
                Some(c.compression.to_string()),
                Some(c.encoding.to_string()),
                Some(yes_no(c.dictionary).to_owned()),
                Some(c.statistics.to_string()),
                Some(yes_no(c.bloom_filter).to_owned()),
            ]
        })
        .collect::<Vec<_>>();
    table::write(out, &header.map(String::from), None, &rows, &[false; 6])?;
    Ok(())
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

pub(crate) fn schema(doc: &Document, out: &mut impl Write) -> Result<(), Error> {
    fn field(out: &mut impl Write, f: &Field, depth: usize) -> std::io::Result<()> {
        let indent = "  ".repeat(depth);
        let required = if f.is_nullable() { "" } else { " (required)" };
        let ty = match f.data_type() {
            DataType::Struct(_) => "struct".to_owned(),
            other => type_name(other),
        };
        writeln!(out, "{indent}{}: {ty}{required}", f.name())?;
        if let DataType::Struct(children) = f.data_type() {
            for child in children {
                field(out, child, depth + 1)?;
            }
        }
        Ok(())
    }
    for f in doc.schema().fields() {
        field(out, f, 0)?;
    }
    Ok(())
}

pub(crate) fn head(doc: &Document, rows: usize, out: &mut impl Write) -> Result<(), Error> {
    let schema = doc.schema();
    let batch = doc.read(0..rows)?;
    let cells = format_batch(&batch)?;
    let names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    let types: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| type_name(f.data_type()))
        .collect();
    let right: Vec<bool> = schema
        .fields()
        .iter()
        .map(|f| veta_fmt::is_numeric(f.data_type()))
        .collect();
    table::write(out, &names, Some(&types), &cells, &right)?;
    if doc.num_rows() > rows {
        writeln!(out, "({} of {} rows)", batch.num_rows(), doc.num_rows())?;
    }
    Ok(())
}

pub(crate) fn stats(
    doc: &Document,
    columns: Vec<String>,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Error> {
    use veta_fmt::{format_distinct, format_float, format_stat_value};
    let stats = doc.stats_job(columns)?.run(&mut |_| true)?;
    if json {
        writeln!(out, "{}", veta_core::stats::to_json(&stats)?)?;
        return Ok(());
    }
    let header: Vec<String> = [
        "column", "type", "count", "nulls", "distinct", "min", "max", "mean", "std dev",
    ]
    .map(String::from)
    .to_vec();
    let rows: Vec<Vec<Option<String>>> = stats
        .iter()
        .map(|s| {
            vec![
                Some(s.column.clone()),
                Some(s.type_name.clone()),
                Some(s.count.to_string()),
                Some(s.nulls.to_string()),
                format_distinct(s),
                s.min.as_ref().map(format_stat_value),
                s.max.as_ref().map(format_stat_value),
                s.mean.map(format_float),
                s.std_dev.map(format_float),
            ]
        })
        .collect();
    let right = [false, false, true, true, true, false, false, true, true];
    table::write(out, &header, None, &rows, &right)?;
    Ok(())
}
