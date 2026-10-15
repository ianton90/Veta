//! `convert`: between Parquet, CSV and JSON.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FileFormat {
    Parquet,
    Csv,
    /// A JSON array of objects.
    Json,
    /// One JSON object per line.
    Ndjson,
}

impl FileFormat {
    fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "parquet" | "parq" | "pq" => Self::Parquet,
            "csv" | "tsv" | "txt" => Self::Csv,
            "json" => Self::Json,
            "ndjson" | "jsonl" => Self::Ndjson,
            _ => return None,
        })
    }
}

/// What to do with CSV values that don't fit their column's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OnErrorArg {
    /// Keep them as text in a "<column> errors" column.
    Column,
    /// Import them as null.
    Null,
    /// Stop with an error.
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InputEncoding {
    Utf8,
    Latin1,
    Windows1252,
    Utf16le,
    Utf16be,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ConvertArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Input format (default: from the input's extension).
    #[arg(long, value_enum)]
    pub from: Option<FileFormat>,
    /// Output format (default: from the output's extension).
    #[arg(long = "format", value_enum)]
    pub to: Option<FileFormat>,
    /// CSV field separator, e.g. ";" or "tab" (default: "," or tab for .tsv).
    #[arg(long)]
    pub delimiter: Option<String>,
    /// CSV quote character, or "none" (default: ").
    #[arg(long)]
    pub quote: Option<String>,
    /// CSV decimal separator.
    #[arg(long, default_value = ".")]
    pub decimal: char,
    /// Format of CSV dates and timestamps, e.g. "%d/%m/%Y" (default: ISO 8601).
    #[arg(long, value_name = "FORMAT")]
    pub date_format: Option<String>,
    /// The CSV input has no header row / don't write one.
    #[arg(long)]
    pub no_header: bool,
    /// CSV input encoding.
    #[arg(long, value_enum, default_value = "utf8")]
    pub encoding: InputEncoding,
    /// CSV values that don't fit their column's type.
    #[arg(long, value_enum, default_value = "column")]
    pub on_error: OnErrorArg,
}

/// Converts a file between formats.
pub fn convert(args: ConvertArgs, out: &mut impl Write) -> Result<(), Error> {
    use veta_core::convert::{
        CsvExportOptions, CsvOptions, ExportFormat, TextEncoding, export, import_csv, import_json,
    };
    let format_of = |given: Option<FileFormat>, path: &Path| {
        given
            .or_else(|| FileFormat::from_path(path))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "can't tell the format of {}; use --from/--format",
                    path.display()
                ))
            })
    };
    let from = format_of(args.from, &args.input)?;
    let to = format_of(args.to, &args.output)?;
    let tab = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
    let delimiter = |path: &Path| -> Result<u8, Error> {
        match args.delimiter.as_deref() {
            None if tab(path) => Ok(b'\t'),
            None => Ok(b','),
            Some("tab" | "\\t") => Ok(b'\t'),
            Some(d) if d.len() == 1 => Ok(d.as_bytes()[0]),
            Some(d) => Err(Error::Usage(format!(
                "the delimiter must be one character, not {d:?}"
            ))),
        }
    };
    let quote = match args.quote.as_deref() {
        None => Some(b'"'),
        Some("none") => None,
        Some(q) if q.len() == 1 => Some(q.as_bytes()[0]),
        Some(q) => {
            return Err(Error::Usage(format!(
                "the quote must be one character or \"none\", not {q:?}"
            )));
        }
    };
    let budget = veta_core::io::DEFAULT_MEMORY_BUDGET;
    let mut doc = match from {
        FileFormat::Parquet => crate::open(&args.input)?,
        FileFormat::Csv => {
            let (doc, report) = import_csv(
                &args.input,
                &CsvOptions {
                    delimiter: delimiter(&args.input)?,
                    quote,
                    header: !args.no_header,
                    encoding: match args.encoding {
                        InputEncoding::Utf8 => TextEncoding::Utf8,
                        InputEncoding::Latin1 => TextEncoding::Latin1,
                        InputEncoding::Windows1252 => TextEncoding::Windows1252,
                        InputEncoding::Utf16le => TextEncoding::Utf16Le,
                        InputEncoding::Utf16be => TextEncoding::Utf16Be,
                    },
                    decimal: args.decimal,
                    date_format: args.date_format.clone(),
                    on_error: match args.on_error {
                        OnErrorArg::Column => veta_core::steps::OnError::ErrorColumn,
                        OnErrorArg::Null => veta_core::steps::OnError::Null,
                        OnErrorArg::Fail => veta_core::steps::OnError::Fail,
                    },
                    ..CsvOptions::default()
                },
                budget,
                &mut |_| true,
            )?;
            if let Some(summary) = report.summary() {
                writeln!(out, "{summary}")?;
            }
            doc
        }
        FileFormat::Json | FileFormat::Ndjson => import_json(&args.input, budget, &mut |_| true)?,
    };
    let format = match to {
        FileFormat::Parquet => {
            doc.save(Some(args.output.clone()))?;
            writeln!(out, "Saved {}", args.output.display())?;
            return Ok(());
        }
        FileFormat::Csv => ExportFormat::Csv(CsvExportOptions {
            delimiter: delimiter(&args.output)?,
            quote,
            header: !args.no_header,
            decimal: args.decimal,
        }),
        FileFormat::Json => ExportFormat::JsonArray,
        FileFormat::Ndjson => ExportFormat::JsonLines,
    };
    export(&doc.output()?, &args.output, &format, &mut |_| true)?;
    writeln!(
        out,
        "Wrote {} rows to {}",
        doc.num_rows(),
        args.output.display()
    )?;
    Ok(())
}
