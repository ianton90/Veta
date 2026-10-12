//! Editing subcommands. Each opens the file, runs core commands and saves,
//! in place or to `--output`.

use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use veta_core::display::parse_type_name;
use veta_core::model::KeyValue;
use veta_core::{
    Command, Compression, Document, Encoding, FormatVersion, StatisticsLevel, controller,
};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct Output {
    /// Write the result here instead of changing the file in place.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum MetaCommand {
    /// List all key/value metadata.
    List { file: PathBuf },
    /// Print one value.
    Get { file: PathBuf, key: String },
    /// Set a value, adding the key if needed.
    Set {
        file: PathBuf,
        key: String,
        value: String,
        #[command(flatten)]
        output: Output,
    },
    /// Remove a key.
    Remove {
        file: PathBuf,
        key: String,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompressionArg {
    Uncompressed,
    Snappy,
    Gzip,
    Brotli,
    Lz4,
    Lz4Raw,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EncodingArg {
    Plain,
    DeltaBinaryPacked,
    DeltaLengthByteArray,
    DeltaByteArray,
    ByteStreamSplit,
    Rle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StatisticsArg {
    None,
    Chunk,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Toggle {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    #[value(name = "1")]
    V1,
    #[value(name = "2")]
    V2,
}

/// Options of `veta-cli writer-settings`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct WriterArgs {
    pub file: PathBuf,
    /// Columns to change (repeatable). Without it, column options apply to
    /// every column and to new columns.
    #[arg(long = "column", value_name = "NAME")]
    pub columns: Vec<String>,
    #[arg(long)]
    pub compression: Option<CompressionArg>,
    /// Compression level (gzip 0-10, brotli 0-11, zstd 1-22).
    #[arg(long)]
    pub level: Option<i32>,
    #[arg(long)]
    pub encoding: Option<EncodingArg>,
    #[arg(long)]
    pub dictionary: Option<Toggle>,
    #[arg(long)]
    pub statistics: Option<StatisticsArg>,
    #[arg(long)]
    pub bloom_filter: Option<Toggle>,
    /// Maximum rows per row group.
    #[arg(long)]
    pub row_group_rows: Option<usize>,
    #[arg(long)]
    pub format_version: Option<FormatArg>,
    #[command(flatten)]
    pub output: Output,
}

impl WriterArgs {
    fn changes_anything(&self) -> bool {
        self.compression.is_some()
            || self.level.is_some()
            || self.encoding.is_some()
            || self.dictionary.is_some()
            || self.statistics.is_some()
            || self.bloom_filter.is_some()
            || self.row_group_rows.is_some()
            || self.format_version.is_some()
    }
}

/// Opens `file`, runs the commands `build` returns, and saves.
pub fn edit(
    file: &Path,
    output: &Output,
    out: &mut impl Write,
    build: impl FnOnce(&Document) -> Result<Vec<Command>, Error>,
) -> Result<(), Error> {
    let mut doc = crate::open(file)?;
    for command in build(&doc)? {
        controller::execute(&mut doc, command)?;
    }
    doc.save(output.output.clone())?;
    let target = output.output.as_deref().unwrap_or(file);
    writeln!(out, "Saved {}", target.display())?;
    Ok(())
}

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
                    },
                    decimal: args.decimal,
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

/// Applies a recipe's steps to `file` and saves the result.
pub fn apply(
    recipe: &Path,
    file: &Path,
    output: &Output,
    out: &mut impl Write,
) -> Result<(), Error> {
    let text = std::fs::read_to_string(recipe)?;
    let mut doc = crate::open(file)?;
    let steps = veta_core::recipe::from_json(&text, &doc.schema())?;
    let count = steps.len();
    if count > 0 {
        controller::execute(&mut doc, Command::AppendSteps { steps })?;
    }
    let stop = |doc: &veta_core::Document, step: usize, error: &dyn std::fmt::Display| {
        Error::Usage(format!(
            "stopped at step {} ({}): {error}",
            step + 1,
            doc.steps()[step].describe()
        ))
    };
    // Run full passes here so a failing step is reported by number.
    while let Some(job) = doc.compute_job() {
        let step = job.step();
        match job.run(&mut |_| true) {
            Ok(computed) => {
                doc.install(computed);
            }
            Err(e) => return Err(stop(&doc, step, &e)),
        }
    }
    if let veta_core::steps::Status::Broken { step, error } = doc.status() {
        return Err(stop(&doc, *step, error));
    }
    doc.save(output.output.clone())?;
    let target = output.output.as_deref().unwrap_or(file);
    writeln!(out, "Applied {count} steps; saved {}", target.display())?;
    Ok(())
}

/// Converts a 1-based row number to an index.
pub fn row_index(row: usize) -> Result<usize, Error> {
    row.checked_sub(1)
        .ok_or_else(|| Error::Usage("row numbers start at 1".into()))
}

/// Parses `5`, `10-20` (inclusive, 1-based) into 0-based ranges.
pub fn parse_rows(specs: &[String]) -> Result<Vec<Range<usize>>, Error> {
    specs
        .iter()
        .flat_map(|s| s.split(','))
        .map(|part| {
            let part = part.trim();
            let bad = || Error::Usage(format!("invalid row range {part:?}; use e.g. 5 or 10-20"));
            let (first, last) = match part.split_once('-') {
                Some((a, b)) => (
                    a.trim().parse().map_err(|_| bad())?,
                    b.trim().parse().map_err(|_| bad())?,
                ),
                None => {
                    let n = part.parse().map_err(|_| bad())?;
                    (n, n)
                }
            };
            if first == 0 || last < first {
                return Err(bad());
            }
            Ok(first - 1..last)
        })
        .collect()
}

pub fn parse_type(name: &str) -> Result<veta_core::arrow::datatypes::DataType, Error> {
    parse_type_name(name).ok_or_else(|| {
        Error::Usage(format!(
            "unknown type {name:?}; use e.g. string, int64, float64, bool, date, timestamp[ms], decimal(10, 2)"
        ))
    })
}

pub fn meta(command: MetaCommand, out: &mut impl Write) -> Result<(), Error> {
    match command {
        MetaCommand::List { file } => {
            let doc = crate::open(&file)?;
            for kv in &doc.metadata().key_value {
                writeln!(out, "{} = {}", kv.key, kv.value.as_deref().unwrap_or(""))?;
            }
            Ok(())
        }
        MetaCommand::Get { file, key } => {
            let doc = crate::open(&file)?;
            match doc.metadata().get(&key) {
                Some(KeyValue { value, .. }) => {
                    writeln!(out, "{}", value.as_deref().unwrap_or(""))?;
                    Ok(())
                }
                None => Err(Error::Usage(format!("no metadata key {key:?}"))),
            }
        }
        MetaCommand::Set {
            file,
            key,
            value,
            output,
        } => edit(&file, &output, out, |_| {
            Ok(vec![Command::SetMetadata {
                key,
                value: Some(value),
            }])
        }),
        MetaCommand::Remove { file, key, output } => edit(&file, &output, out, |_| {
            Ok(vec![Command::RemoveMetadata { key }])
        }),
    }
}

pub fn writer_settings(args: WriterArgs, out: &mut impl Write) -> Result<(), Error> {
    if !args.changes_anything() {
        let doc = crate::open(&args.file)?;
        return crate::print_writer_settings(&doc, out);
    }
    if args.level.is_some() && args.compression.is_none() {
        return Err(Error::Usage("--level needs --compression".into()));
    }
    let compression = args
        .compression
        .map(|c| compression(c, args.level))
        .transpose()?;
    let output = args.output.clone();
    edit(&args.file.clone(), &output, out, |doc| {
        let schema = doc.schema();
        let mut settings = doc.writer_settings().clone();
        if let Some(rows) = args.row_group_rows {
            settings.max_row_group_rows = rows;
        }
        if let Some(version) = args.format_version {
            settings.format_version = match version {
                FormatArg::V1 => FormatVersion::V1,
                FormatArg::V2 => FormatVersion::V2,
            };
        }
        let targets: Vec<String> = if args.columns.is_empty() {
            schema.fields().iter().map(|f| f.name().clone()).collect()
        } else {
            args.columns.clone()
        };
        let encoding = args.encoding.map(encoding);
        for name in &targets {
            let field = schema
                .field_with_name(name)
                .map_err(|_| Error::Usage(format!("no column named {name:?}")))?;
            let mut column = *settings.column(name);
            if let Some(c) = compression {
                column.compression = c;
            }
            if let Some(e) = encoding {
                if Encoding::choices_for(field.data_type()).contains(&e) {
                    column.encoding = e;
                } else if !args.columns.is_empty() {
                    return Err(Error::Usage(format!(
                        "{e} encoding does not apply to column {name}"
                    )));
                }
            }
            if let Some(d) = args.dictionary {
                column.dictionary = d == Toggle::On;
            }
            if let Some(s) = args.statistics {
                column.statistics = statistics(s);
            }
            if let Some(b) = args.bloom_filter {
                column.bloom_filter = b == Toggle::On;
            }
            match settings.columns.iter_mut().find(|(p, _)| p == name) {
                Some((_, existing)) => *existing = column,
                None => settings.columns.push((name.clone(), column)),
            }
        }
        if args.columns.is_empty() {
            let d = &mut settings.default_column;
            if let Some(c) = compression {
                d.compression = c;
            }
            if let Some(v) = args.dictionary {
                d.dictionary = v == Toggle::On;
            }
            if let Some(s) = args.statistics {
                d.statistics = statistics(s);
            }
            if let Some(b) = args.bloom_filter {
                d.bloom_filter = b == Toggle::On;
            }
        }
        Ok(vec![Command::SetWriterSettings(settings)])
    })
}

fn compression(c: CompressionArg, level: Option<i32>) -> Result<Compression, Error> {
    let unsigned = |level: Option<i32>| {
        level
            .map(|l| u32::try_from(l).map_err(|_| Error::Usage("level must be 0 or more".into())))
            .transpose()
    };
    let no_level = |c: Compression| match level {
        Some(_) => Err(Error::Usage(format!("{c} has no compression levels"))),
        None => Ok(c),
    };
    match c {
        CompressionArg::Uncompressed => no_level(Compression::Uncompressed),
        CompressionArg::Snappy => no_level(Compression::Snappy),
        CompressionArg::Lz4 => no_level(Compression::Lz4),
        CompressionArg::Lz4Raw => no_level(Compression::Lz4Raw),
        CompressionArg::Gzip => Ok(Compression::Gzip(unsigned(level)?)),
        CompressionArg::Brotli => Ok(Compression::Brotli(unsigned(level)?)),
        CompressionArg::Zstd => Ok(Compression::Zstd(level)),
    }
}

fn encoding(e: EncodingArg) -> Encoding {
    match e {
        EncodingArg::Plain => Encoding::Plain,
        EncodingArg::DeltaBinaryPacked => Encoding::DeltaBinaryPacked,
        EncodingArg::DeltaLengthByteArray => Encoding::DeltaLengthByteArray,
        EncodingArg::DeltaByteArray => Encoding::DeltaByteArray,
        EncodingArg::ByteStreamSplit => Encoding::ByteStreamSplit,
        EncodingArg::Rle => Encoding::Rle,
    }
}

fn statistics(s: StatisticsArg) -> StatisticsLevel {
    match s {
        StatisticsArg::None => StatisticsLevel::None,
        StatisticsArg::Chunk => StatisticsLevel::Chunk,
        StatisticsArg::Page => StatisticsLevel::Page,
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Row ranges.
mod tests {
    use super::*;

    #[test]
    fn row_specs() {
        let specs = vec!["5".to_owned(), "10-12, 1".to_owned()];
        assert_eq!(parse_rows(&specs).unwrap(), vec![4..5, 9..12, 0..1]);
        assert!(parse_rows(&["0".to_owned()]).is_err());
        assert!(parse_rows(&["5-3".to_owned()]).is_err());
        assert!(parse_rows(&["x".to_owned()]).is_err());
    }
}
