//! Importing CSV and JSON into documents, and exporting a document's output
//! to CSV or JSON.
//!
//! Imports stream the input in batches into a temporary Parquet file; small
//! results are then loaded into memory, larger ones stay paged from that
//! file (deleted when the document is closed). Exports stream the
//! evaluated output (after all steps) to a temporary file next to the
//! target, renamed into place when complete.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ParquetRecordBatchReaderBuilder};

use crate::error::{Error, Result};
use crate::model::Document;
use crate::source::{DataSource, MemorySource};
use crate::steps::checkpoint::{Checkpoint, TempFile};
use crate::steps::{Cast, CastOptions, OnError, Pipeline};

/// Rows per batch when reading or writing.
const BATCH_ROWS: usize = 64 * 1024;
/// Rows read to guess column types.
pub const INFER_ROWS: usize = 1000;

/// Text encodings for CSV files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    /// UTF-8; files starting with a UTF-16 byte order mark are read as
    /// UTF-16.
    #[default]
    Utf8,
    /// ISO-8859-1.
    Latin1,
    /// Windows-1252, Windows' "ANSI" for Western languages (Latin-1 plus
    /// characters such as € and curly quotes).
    Windows1252,
    Utf16Le,
    Utf16Be,
}

impl TextEncoding {
    pub const ALL: [TextEncoding; 5] = [
        Self::Utf8,
        Self::Latin1,
        Self::Windows1252,
        Self::Utf16Le,
        Self::Utf16Be,
    ];
}

impl std::fmt::Display for TextEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Utf8 => "UTF-8",
            Self::Latin1 => "Latin-1 (ISO-8859-1)",
            Self::Windows1252 => "Windows-1252",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
        })
    }
}

/// Windows-1252 characters for bytes 0x80-0x9F (the rest match Latin-1).
const WINDOWS_1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

/// How to read a CSV file.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvOptions {
    pub delimiter: u8,
    /// Character around values that contain separators; `None` reads
    /// quotes as ordinary characters.
    pub quote: Option<u8>,
    /// The first row holds column names.
    pub header: bool,
    pub encoding: TextEncoding,
    /// Decimal separator in numbers.
    pub decimal: char,
    /// Column types; `None` guesses them from the first rows.
    pub types: Option<Vec<DataType>>,
    /// Format of date and timestamp columns (`%d/%m/%Y`, …, as in Change
    /// type); `None` reads ISO 8601.
    pub date_format: Option<String>,
    /// What to do with values that don't fit their column's type.
    pub on_error: OnError,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: Some(b'"'),
            header: true,
            encoding: TextEncoding::Utf8,
            decimal: '.',
            types: None,
            date_format: None,
            on_error: OnError::ErrorColumn,
        }
    }
}

/// The first rows of a CSV file, as they would be imported.
#[derive(Debug, Clone)]
pub struct CsvPreview {
    /// Column names and types (guessed unless given).
    pub schema: SchemaRef,
    /// The rows, converted to those types.
    pub rows: RecordBatch,
    /// Per column, how many of the previewed values don't fit its type.
    pub failures: Vec<usize>,
}

/// How to write a CSV file.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvExportOptions {
    pub delimiter: u8,
    /// Character around values that need it; `None` never quotes.
    pub quote: Option<u8>,
    pub header: bool,
    /// Decimal separator for numbers.
    pub decimal: char,
}

impl Default for CsvExportOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: Some(b'"'),
            header: true,
            decimal: '.',
        }
    }
}

/// Export formats.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportFormat {
    Csv(CsvExportOptions),
    /// One JSON object per line.
    JsonLines,
    /// A single JSON array of objects.
    JsonArray,
}

/// Formats files can be imported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Csv,
    Json,
}

impl ImportFormat {
    /// Guesses the format from a file name.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "csv" | "tsv" | "txt" => Some(Self::Csv),
            "json" | "ndjson" | "jsonl" => Some(Self::Json),
            _ => None,
        }
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidCommand(message.into())
}

// Reading text.

/// Reads a file as UTF-8, transcoding Latin-1 and dropping a UTF-8 byte
/// order mark; counts bytes and stops when `progress` says to.
struct Input<'a> {
    inner: BufReader<File>,
    encoding: TextEncoding,
    read: u64,
    total: u64,
    progress: Option<&'a mut dyn FnMut(f32) -> bool>,
    cancelled: bool,
    /// Transcoded bytes not yet returned.
    pending: Vec<u8>,
    /// UTF-16 bytes kept for the next chunk (an odd byte, or the first
    /// half of a surrogate pair).
    carry: Vec<u8>,
    started: bool,
}

impl<'a> Input<'a> {
    fn open(
        path: &Path,
        encoding: TextEncoding,
        progress: Option<&'a mut dyn FnMut(f32) -> bool>,
    ) -> Result<Self> {
        let file = File::open(path)?;
        let total = file.metadata()?.len();
        Ok(Self {
            inner: BufReader::with_capacity(1 << 16, file),
            encoding,
            read: 0,
            total,
            progress,
            cancelled: false,
            pending: Vec::new(),
            carry: Vec::new(),
            started: false,
        })
    }
}

impl Read for Input<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.pending.is_empty() {
            let n = buf.len().min(self.pending.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        let chunk = self.inner.fill_buf()?;
        let mut chunk = chunk.to_vec();
        let len = chunk.len();
        self.inner.consume(len);
        if len == 0 {
            if self.carry.is_empty() {
                return Ok(0);
            }
            // A dangling UTF-16 byte or surrogate at the end.
            let rest = std::mem::take(&mut self.carry);
            self.pending = decode_utf16(&rest, self.encoding == TextEncoding::Utf16Be, true).0;
            return self.read(buf);
        }
        self.read += len as u64;
        if let Some(progress) = &mut self.progress
            && !progress(self.read as f32 / self.total.max(1) as f32)
        {
            self.cancelled = true;
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if !self.started {
            self.started = true;
            if self.encoding == TextEncoding::Utf8 && chunk.starts_with(&[0xEF, 0xBB, 0xBF]) {
                chunk.drain(..3);
            }
            // A UTF-16 byte order mark picks the byte order.
            let utf16 = matches!(
                self.encoding,
                TextEncoding::Utf8 | TextEncoding::Utf16Le | TextEncoding::Utf16Be
            );
            if utf16 && chunk.starts_with(&[0xFF, 0xFE]) {
                self.encoding = TextEncoding::Utf16Le;
                chunk.drain(..2);
            } else if utf16 && chunk.starts_with(&[0xFE, 0xFF]) {
                self.encoding = TextEncoding::Utf16Be;
                chunk.drain(..2);
            }
        }
        self.pending = match self.encoding {
            TextEncoding::Utf8 => chunk,
            TextEncoding::Latin1 => chunk
                .iter()
                .map(|&b| b as char)
                .collect::<String>()
                .into_bytes(),
            TextEncoding::Windows1252 => chunk
                .iter()
                .map(|&b| match b {
                    0x80..=0x9F => WINDOWS_1252[usize::from(b - 0x80)],
                    _ => b as char,
                })
                .collect::<String>()
                .into_bytes(),
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                let mut bytes = std::mem::take(&mut self.carry);
                bytes.extend_from_slice(&chunk);
                let (text, rest) =
                    decode_utf16(&bytes, self.encoding == TextEncoding::Utf16Be, false);
                self.carry = rest;
                text
            }
        };
        self.read(buf)
    }
}

/// Decodes UTF-16 `bytes` to UTF-8, returning the bytes to keep for the
/// next chunk: an odd last byte, or a high surrogate waiting for its pair
/// (`at_end`: there is no next chunk).
/// Invalid units become U+FFFD.
fn decode_utf16(bytes: &[u8], big_endian: bool, at_end: bool) -> (Vec<u8>, Vec<u8>) {
    let even = bytes.len() - bytes.len() % 2;
    let mut units: Vec<u16> = bytes[..even]
        .chunks_exact(2)
        .map(|p| {
            if big_endian {
                u16::from_be_bytes([p[0], p[1]])
            } else {
                u16::from_le_bytes([p[0], p[1]])
            }
        })
        .collect();
    let mut rest = bytes[even..].to_vec();
    if !at_end && units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
        let unit = units.pop().unwrap_or_default();
        let pair = if big_endian {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        };
        rest.splice(0..0, pair);
    }
    let text: String = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    (text.into_bytes(), rest)
}

/// Turns a JSON array of objects into one object per line.
struct ArrayToLines<R> {
    inner: R,
    /// Inside the outer `[`.
    opened: bool,
    depth: usize,
    in_string: bool,
    escaped: bool,
    pending: Vec<u8>,
}

impl<R: Read> ArrayToLines<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            opened: false,
            depth: 0,
            in_string: false,
            escaped: false,
            pending: Vec::new(),
        }
    }
}

impl<R: Read> Read for ArrayToLines<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() {
            let mut chunk = [0u8; 1 << 16];
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                return Ok(0);
            }
            for &b in &chunk[..n] {
                if self.depth == 0 {
                    match b {
                        b'[' if !self.opened => self.opened = true,
                        b'{' if self.opened => {
                            self.depth = 1;
                            self.pending.push(b);
                        }
                        b',' | b']' | b' ' | b'\t' | b'\r' | b'\n' if self.opened => {}
                        b' ' | b'\t' | b'\r' | b'\n' | 0xEF | 0xBB | 0xBF => {}
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "expected a JSON array of objects",
                            ));
                        }
                    }
                    continue;
                }
                self.pending.push(b);
                if self.in_string {
                    match (self.escaped, b) {
                        (true, _) => self.escaped = false,
                        (false, b'\\') => self.escaped = true,
                        (false, b'"') => self.in_string = false,
                        _ => {}
                    }
                    continue;
                }
                match b {
                    b'"' => self.in_string = true,
                    b'{' | b'[' => self.depth += 1,
                    b'}' | b']' => {
                        self.depth -= 1;
                        if self.depth == 0 {
                            self.pending.push(b'\n');
                        }
                    }
                    _ => {}
                }
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

// CSV.

fn csv_format(options: &CsvOptions) -> arrow::csv::reader::Format {
    arrow::csv::reader::Format::default()
        .with_header(options.header)
        .with_delimiter(options.delimiter)
        // No quoting: a byte that doesn't occur in text files.
        .with_quote(options.quote.unwrap_or(0))
        .with_truncated_rows(true)
}

/// Converts text columns to the types of `schema`. Returns the converted
/// columns, the rows whose value didn't fit, per column, and the original
/// text of those values.
fn convert_columns(
    batch: &RecordBatch,
    schema: &SchemaRef,
    options: &CsvOptions,
) -> Result<Vec<(ArrayRef, Vec<usize>)>> {
    batch
        .columns()
        .iter()
        .zip(schema.fields())
        .map(|(column, field)| {
            if field.data_type() == &DataType::Utf8 {
                return Ok((column.clone(), Vec::new()));
            }
            let converted = Cast {
                column: field.name().clone(),
                to: field.data_type().clone(),
                options: CastOptions {
                    decimal: options.decimal,
                    thousands: None,
                    format: options.date_format.clone().filter(|_| {
                        matches!(
                            field.data_type(),
                            DataType::Date32 | DataType::Timestamp(..)
                        )
                    }),
                    on_error: OnError::Null,
                },
            }
            .convert(column)?;
            let failed = (0..column.len())
                .filter(|&i| column.is_valid(i) && converted.is_null(i))
                .collect();
            Ok((converted, failed))
        })
        .collect()
}

/// Name of the column holding a column's invalid values.
pub fn error_column_name(column: &str) -> String {
    format!("{column} errors")
}

/// The imported layout: typed columns, each followed (with error columns)
/// by a text column of the values that didn't fit. Returns the schema and,
/// per output column, the input column it comes from and whether it is an
/// error column.
fn import_layout(schema: &SchemaRef, on_error: OnError) -> (SchemaRef, Vec<(usize, bool)>) {
    let mut fields = Vec::new();
    let mut layout = Vec::new();
    for (i, field) in schema.fields().iter().enumerate() {
        fields.push(field.as_ref().clone());
        layout.push((i, false));
        if on_error == OnError::ErrorColumn && field.data_type() != &DataType::Utf8 {
            let mut name = error_column_name(field.name());
            while schema.fields().iter().any(|f| f.name() == &name) {
                name.push('_');
            }
            fields.push(Field::new(name, DataType::Utf8, true));
            layout.push((i, true));
        }
    }
    (Arc::new(Schema::new(fields)), layout)
}

/// Converts one batch of text into the imported layout, counting failures
/// per input column; with [`OnError::Fail`], the first failure is an error.
fn import_batch(
    text: &RecordBatch,
    schema: &SchemaRef,
    layout: &(SchemaRef, Vec<(usize, bool)>),
    options: &CsvOptions,
    first_row: usize,
    failures: &mut [u64],
) -> Result<RecordBatch> {
    let converted = convert_columns(text, schema, options)?;
    for (i, (_, failed)) in converted.iter().enumerate() {
        failures[i] += failed.len() as u64;
        if options.on_error == OnError::Fail
            && let Some(&row) = failed.first()
        {
            let value = array_text(text.column(i), row);
            return Err(invalid(format!(
                "row {}: {value:?} in column {} is not a valid {}",
                first_row + row + 1,
                schema.field(i).name(),
                crate::display::type_name(schema.field(i).data_type())
            )));
        }
    }
    let columns = layout
        .1
        .iter()
        .map(|&(i, error)| {
            if !error {
                return converted[i].0.clone();
            }
            let failed = &converted[i].1;
            let mut values: Vec<Option<String>> = vec![None; text.num_rows()];
            for &row in failed {
                values[row] = Some(array_text(text.column(i), row));
            }
            Arc::new(StringArray::from(values)) as ArrayRef
        })
        .collect();
    Ok(RecordBatch::try_new(layout.0.clone(), columns)?)
}

fn array_text(array: &ArrayRef, row: usize) -> String {
    array
        .as_any()
        .downcast_ref::<StringArray>()
        .filter(|a| a.is_valid(row))
        .map(|a| a.value(row).to_owned())
        .unwrap_or_default()
}

/// What an import found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub rows: u64,
    /// Columns with values that didn't fit their type, and how many.
    pub failures: Vec<(String, u64)>,
    /// What happened to them.
    pub on_error: Option<OnError>,
}

impl ImportReport {
    /// A sentence for the user, if any values didn't fit.
    pub fn summary(&self) -> Option<String> {
        if self.failures.is_empty() {
            return None;
        }
        let total: u64 = self.failures.iter().map(|(_, n)| n).sum();
        let columns: Vec<String> = self
            .failures
            .iter()
            .map(|(c, n)| format!("{c} ({n})"))
            .collect();
        let fate = match self.on_error {
            Some(OnError::ErrorColumn) => "kept as text in the matching \"errors\" columns",
            _ => "imported as null",
        };
        Some(format!(
            "{total} values didn't fit their column's type and were {fate}: {}.",
            columns.join(", ")
        ))
    }
}

fn text_schema(schema: &Schema) -> SchemaRef {
    Arc::new(Schema::new(
        schema
            .fields()
            .iter()
            .map(|f| Field::new(f.name(), DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ))
}

/// Column names and guessed types.
fn infer_csv(path: &Path, options: &CsvOptions) -> Result<SchemaRef> {
    let input = Input::open(path, options.encoding, None)?;
    let (schema, _) = csv_format(options).infer_schema(input, Some(INFER_ROWS))?;
    let mut fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|f| Field::new(f.name(), f.data_type().clone(), true))
        .collect();
    // With a decimal comma, numbers look like text to arrow: check the
    // sample ourselves.
    if options.decimal != '.' && fields.iter().any(|f| f.data_type() == &DataType::Utf8) {
        let sample = read_csv_text(path, options, &text_schema(&schema), INFER_ROWS)?;
        for (i, field) in fields.iter_mut().enumerate() {
            if field.data_type() != &DataType::Utf8 {
                continue;
            }
            let column = sample.column(i);
            let cast = Cast {
                column: field.name().clone(),
                to: DataType::Float64,
                options: CastOptions {
                    decimal: options.decimal,
                    ..CastOptions::default()
                },
            };
            let preview = cast.preview(column);
            if column.len() > column.null_count() && preview.failed == 0 {
                *field = Field::new(field.name(), DataType::Float64, true);
            }
        }
    }
    Ok(Arc::new(Schema::new(fields)))
}

/// The first `rows` rows of the file, all as text.
fn read_csv_text(
    path: &Path,
    options: &CsvOptions,
    schema: &SchemaRef,
    rows: usize,
) -> Result<RecordBatch> {
    let input = Input::open(path, options.encoding, None)?;
    let mut reader = arrow::csv::ReaderBuilder::new(schema.clone())
        .with_format(csv_format(options))
        .with_batch_size(rows.max(1))
        .build(input)?;
    match reader.next() {
        Some(batch) => Ok(batch?),
        None => Ok(RecordBatch::new_empty(schema.clone())),
    }
}

/// The schema a CSV file will be imported with.
fn csv_schema(path: &Path, options: &CsvOptions) -> Result<SchemaRef> {
    let inferred = infer_csv(path, options)?;
    match &options.types {
        None => Ok(inferred),
        Some(types) => {
            if types.len() != inferred.fields().len() {
                return Err(invalid(format!(
                    "the file has {} columns, not {}",
                    inferred.fields().len(),
                    types.len()
                )));
            }
            Ok(Arc::new(Schema::new(
                inferred
                    .fields()
                    .iter()
                    .zip(types)
                    .map(|(f, t)| Field::new(f.name(), t.clone(), true))
                    .collect::<Vec<_>>(),
            )))
        }
    }
}

/// The first `rows` rows of a CSV file as they would be imported.
pub fn preview_csv(path: &Path, options: &CsvOptions, rows: usize) -> Result<CsvPreview> {
    let schema = csv_schema(path, options)?;
    let text = read_csv_text(path, options, &text_schema(&schema), rows)?;
    let converted = convert_columns(&text, &schema, options)?;
    let failures = converted.iter().map(|(_, f)| f.len()).collect();
    let columns = converted.into_iter().map(|(c, _)| c).collect();
    let rows = RecordBatch::try_new(schema.clone(), columns)?;
    Ok(CsvPreview {
        schema,
        rows,
        failures,
    })
}

/// Imports a CSV file as a new, unsaved document, with a report of the
/// values that didn't fit their column's type. Results larger than
/// `memory_budget` bytes stay on disk and are paged.
pub fn import_csv(
    path: &Path,
    options: &CsvOptions,
    memory_budget: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<(Document, ImportReport)> {
    let schema = csv_schema(path, options)?;
    let layout = import_layout(&schema, options.on_error);
    let mut failures = vec![0u64; schema.fields().len()];
    let mut rows = 0usize;
    let mut input = Input::open(path, options.encoding, Some(progress))?;
    let source = {
        let reader = arrow::csv::ReaderBuilder::new(text_schema(&schema))
            .with_format(csv_format(options))
            .with_batch_size(BATCH_ROWS)
            .build(&mut input)?;
        let batches = reader.map(|text| {
            let text = text?;
            let batch = import_batch(&text, &schema, &layout, options, rows, &mut failures)?;
            rows += text.num_rows();
            Ok(batch)
        });
        store(&layout.0, batches, memory_budget)
    };
    let source = cancelled(source, input.cancelled)?;
    // Error columns of columns where everything fitted are left out.
    let keep: Vec<usize> = layout
        .1
        .iter()
        .enumerate()
        .filter(|(_, (i, error))| !error || failures[*i] > 0)
        .map(|(k, _)| k)
        .collect();
    let source = if keep.len() == layout.1.len() {
        source
    } else {
        Arc::new(Projected::new(source, keep)?)
    };
    let report = ImportReport {
        rows: rows as u64,
        failures: schema
            .fields()
            .iter()
            .zip(&failures)
            .filter(|(_, n)| **n > 0)
            .map(|(f, n)| (f.name().clone(), *n))
            .collect(),
        on_error: Some(options.on_error),
    };
    Ok((Document::imported(file_name(path), source), report))
}

/// Some of a source's columns.
#[derive(Debug)]
struct Projected {
    inner: Arc<dyn DataSource>,
    schema: SchemaRef,
    columns: Vec<usize>,
}

impl Projected {
    fn new(inner: Arc<dyn DataSource>, columns: Vec<usize>) -> Result<Self> {
        let schema = Arc::new(inner.schema().project(&columns)?);
        Ok(Self {
            inner,
            schema,
            columns,
        })
    }
}

impl DataSource for Projected {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.inner.num_rows()
    }

    fn mode(&self) -> crate::source::SourceMode {
        self.inner.mode()
    }

    fn read(&self, range: std::ops::Range<usize>) -> Result<RecordBatch> {
        Ok(self.inner.read(range)?.project(&self.columns)?)
    }
}

// JSON.

fn json_input<'a>(
    path: &Path,
    progress: Option<&'a mut dyn FnMut(f32) -> bool>,
) -> Result<(Box<dyn Read + 'a>, bool)> {
    let mut probe = BufReader::new(File::open(path)?);
    let array = probe
        .fill_buf()?
        .iter()
        .find(|b| !b.is_ascii_whitespace() && ![0xEF, 0xBB, 0xBF].contains(*b))
        == Some(&b'[');
    let input = Input::open(path, TextEncoding::Utf8, progress)?;
    Ok(if array {
        (Box::new(ArrayToLines::new(input)), true)
    } else {
        (Box::new(input), false)
    })
}

/// Column names and types guessed from all of a JSON file's objects (so
/// keys that first appear late aren't dropped), in the order keys first
/// appear. This reads the file once more than the import itself.
fn json_schema(path: &Path) -> Result<SchemaRef> {
    let (input, _) = json_input(path, None)?;
    let (schema, _) = arrow::json::reader::infer_json_schema(BufReader::new(input), None)?;
    let (input, _) = json_input(path, None)?;
    let order = key_order(BufReader::new(input), usize::MAX);
    let mut fields: Vec<_> = schema.fields().iter().cloned().collect();
    fields.sort_by_key(|f| {
        order
            .iter()
            .position(|k| k == f.name())
            .unwrap_or(usize::MAX)
    });
    Ok(Arc::new(Schema::new(fields)))
}

/// Top-level keys of the first `rows` objects, in first-seen order.
fn key_order(input: impl Read, rows: usize) -> Vec<String> {
    struct Keys<'a>(&'a mut Vec<String>);

    impl<'de> serde::de::DeserializeSeed<'de> for Keys<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            d: D,
        ) -> std::result::Result<(), D::Error> {
            d.deserialize_any(self)
        }
    }

    impl<'de> serde::de::Visitor<'de> for Keys<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<(), A::Error> {
            while let Some(key) = map.next_key::<String>()? {
                map.next_value::<serde::de::IgnoredAny>()?;
                if !self.0.contains(&key) {
                    self.0.push(key);
                }
            }
            Ok(())
        }
    }

    let mut keys = Vec::new();
    let mut stream = serde_json::Deserializer::from_reader(input);
    for _ in 0..rows {
        if serde::de::DeserializeSeed::deserialize(Keys(&mut keys), &mut stream).is_err() {
            break;
        }
    }
    keys
}

/// The first `rows` rows of a JSON file.
pub fn preview_json(path: &Path, rows: usize) -> Result<RecordBatch> {
    let schema = json_schema(path)?;
    let (input, _) = json_input(path, None)?;
    let mut reader = arrow::json::ReaderBuilder::new(schema.clone())
        .with_batch_size(rows.max(1))
        .build(BufReader::new(input))?;
    match reader.next() {
        Some(batch) => Ok(batch?),
        None => Ok(RecordBatch::new_empty(schema)),
    }
}

/// Imports a JSON file (one object per line, or an array of objects) as a
/// new, unsaved document. Types are guessed from all objects.
pub fn import_json(
    path: &Path,
    memory_budget: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Document> {
    let schema = json_schema(path)?;
    let mut cancelled_flag = false;
    let source = {
        let mut progress = |f: f32| {
            let go = progress(f);
            if !go {
                cancelled_flag = true;
            }
            go
        };
        let (input, _) = json_input(path, Some(&mut progress))?;
        let reader = arrow::json::ReaderBuilder::new(schema.clone())
            .with_batch_size(BATCH_ROWS)
            .build(BufReader::new(input))?;
        store(
            &schema,
            reader.map(|b| b.map_err(Error::from)),
            memory_budget,
        )
    };
    let source = cancelled(source, cancelled_flag)?;
    Ok(Document::imported(file_name(path), source))
}

// Storing imported batches.

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported".into())
}

fn cancelled<T>(result: Result<T>, was_cancelled: bool) -> Result<T> {
    match result {
        Err(_) if was_cancelled => Err(Error::Cancelled),
        other => other,
    }
}

/// Writes `batches` to a temporary Parquet file, then loads it into memory
/// if it fits `memory_budget`, or keeps it paged.
fn store(
    schema: &SchemaRef,
    batches: impl Iterator<Item = Result<RecordBatch>>,
    memory_budget: usize,
) -> Result<Arc<dyn DataSource>> {
    let file = TempFile::new()?;
    let mut writer = ArrowWriter::try_new(File::create(file.path())?, schema.clone(), None)?;
    for batch in batches {
        writer.write(&batch?)?;
    }
    writer.close()?;
    let handle = File::open(file.path())?;
    let metadata = ArrowReaderMetadata::load(&handle, Default::default())?;
    let size: i64 = metadata
        .metadata()
        .row_groups()
        .iter()
        .map(|rg| rg.total_byte_size())
        .sum();
    if usize::try_from(size).is_ok_and(|s| s <= memory_budget) {
        let reader =
            ParquetRecordBatchReaderBuilder::new_with_metadata(handle, metadata).build()?;
        let batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
        return Ok(Arc::new(MemorySource::new(schema.clone(), batches)?));
    }
    drop(handle);
    Ok(Arc::new(Checkpoint::open(file, schema.clone())?))
}

// Export.

/// Writes the pipeline's output to `target` in `format`.
pub fn export(
    output: &Pipeline,
    target: &Path,
    format: &ExportFormat,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty());
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    let temp = dir
        .unwrap_or(Path::new("."))
        .join(format!(".{name}.veta-{}.tmp", std::process::id()));
    let result = write_export(output, &temp, format, progress)
        .and_then(|()| std::fs::rename(&temp, target).map_err(Into::into));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn write_export(
    output: &Pipeline,
    path: &Path,
    format: &ExportFormat,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let file = BufWriter::new(File::create(path)?);
    let rows = output.num_rows();
    let mut batches = (0..rows.max(1)).step_by(BATCH_ROWS).map(|start| {
        let end = (start + BATCH_ROWS).min(rows);
        output.read(start..end)
    });
    let mut done = 0;
    let mut tick = |n: usize| {
        done += n;
        if progress(done as f32 / rows.max(1) as f32) {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    };
    match format {
        ExportFormat::Csv(options) => {
            let mut builder = arrow::csv::WriterBuilder::new()
                .with_header(options.header)
                .with_delimiter(options.delimiter);
            builder = match options.quote {
                Some(quote) => builder.with_quote(quote),
                None => builder.with_quote_style(arrow::csv::QuoteStyle::Never),
            };
            let mut writer = builder.build(file);
            for batch in batches.by_ref() {
                let batch = decimal_text(&batch?, options.decimal)?;
                writer.write(&batch)?;
                tick(batch.num_rows())?;
            }
            writer.into_inner().flush()?;
        }
        ExportFormat::JsonLines => {
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow::json::writer::LineDelimited>(file);
            for batch in batches.by_ref() {
                let batch = batch?;
                writer.write(&batch)?;
                tick(batch.num_rows())?;
            }
            writer.finish()?;
            writer.into_inner().flush()?;
        }
        ExportFormat::JsonArray => {
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow::json::writer::JsonArray>(file);
            for batch in batches.by_ref() {
                let batch = batch?;
                writer.write(&batch)?;
                tick(batch.num_rows())?;
            }
            writer.finish()?;
            writer.into_inner().flush()?;
        }
    }
    Ok(())
}

/// With a decimal separator other than `.`, writes numbers as text using it.
fn decimal_text(batch: &RecordBatch, decimal: char) -> Result<RecordBatch> {
    if decimal == '.' {
        return Ok(batch.clone());
    }
    let mut fields = Vec::new();
    let mut columns = Vec::new();
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        let t = field.data_type();
        let fractional = t.is_floating()
            || matches!(
                t,
                DataType::Decimal32(..)
                    | DataType::Decimal64(..)
                    | DataType::Decimal128(..)
                    | DataType::Decimal256(..)
            );
        if fractional {
            let text = arrow::compute::cast(column, &DataType::Utf8)?;
            let text = text
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| invalid("expected text"))?;
            let replaced: StringArray = text
                .iter()
                .map(|v| v.map(|v| v.replace('.', &decimal.to_string())))
                .collect();
            fields.push(Field::new(
                field.name(),
                DataType::Utf8,
                field.is_nullable(),
            ));
            columns.push(Arc::new(replaced) as ArrayRef);
        } else {
            fields.push(field.as_ref().clone());
            columns.push(column.clone());
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::format_batch;
    use veta_testkit::TempDir;

    fn cells(doc: &Document) -> Vec<Vec<Option<String>>> {
        format_batch(&doc.read(0..doc.num_rows()).unwrap()).unwrap()
    }

    fn types(doc: &Document) -> Vec<DataType> {
        doc.schema()
            .fields()
            .iter()
            .map(|f| f.data_type().clone())
            .collect()
    }

    #[test]
    fn csv_import_guesses_types() {
        let dir = TempDir::new();
        let path = dir.join("people.csv");
        std::fs::write(
            &path,
            "\u{feff}id,name,score,ok,when\n1,Ana,1.5,true,2024-01-02\n2,\"Bo, Jr.\",,false,2024-02-03\n3,Cy,2,true,\n",
        )
        .unwrap();
        let doc = import_csv(&path, &CsvOptions::default(), 1 << 30, &mut |_| true)
            .unwrap()
            .0;
        assert_eq!(doc.title(), "people.csv");
        assert!(doc.is_modified() && doc.path().is_none());
        assert_eq!(
            types(&doc),
            [
                DataType::Int64,
                DataType::Utf8,
                DataType::Float64,
                DataType::Boolean,
                DataType::Date32
            ]
        );
        let c = cells(&doc);
        assert_eq!(c[1][1].as_deref(), Some("Bo, Jr."));
        assert_eq!(c[1][2], None);
        assert_eq!(c[2][4], None);
    }

    #[test]
    fn csv_options_and_type_overrides() {
        let dir = TempDir::new();
        let path = dir.join("eu.csv");
        // Latin-1 "é" and decimal commas, semicolon separated, no header.
        let mut bytes = b"1;Caf".to_vec();
        bytes.push(0xE9);
        bytes.extend_from_slice(b";1,5\n2;X;abc\n");
        std::fs::write(&path, bytes).unwrap();
        let mut options = CsvOptions {
            delimiter: b';',
            header: false,
            encoding: TextEncoding::Latin1,
            decimal: ',',
            ..CsvOptions::default()
        };
        let preview = preview_csv(&path, &options, 10).unwrap();
        // "abc" makes the third column text.
        assert_eq!(preview.schema.field(2).data_type(), &DataType::Utf8);
        options.types = Some(vec![DataType::Utf8, DataType::Utf8, DataType::Float64]);
        let doc = import_csv(&path, &options, 1 << 30, &mut |_| true)
            .unwrap()
            .0;
        let c = cells(&doc);
        assert_eq!(c[0][1].as_deref(), Some("Café"));
        assert_eq!(c[0][2].as_deref(), Some("1.5"));
        // The invalid value is kept as text in an error column.
        assert_eq!(c[1][2], None);
        assert_eq!(doc.schema().field(3).name(), "column_3 errors");
        assert_eq!(c[1][3].as_deref(), Some("abc"));
        options.types = Some(vec![DataType::Utf8]);
        assert!(import_csv(&path, &options, 1 << 30, &mut |_| true).is_err());
    }

    #[test]
    fn invalid_values_are_reported() {
        let dir = TempDir::new();
        let path = dir.join("bad.csv");
        std::fs::write(&path, "n,m,s\n1,2,a\nx,3,b\n4,y,c\n5,6,d\n").unwrap();
        let types = Some(vec![DataType::Int64, DataType::Int64, DataType::Utf8]);
        let run = |on_error| {
            let options = CsvOptions {
                types: types.clone(),
                on_error,
                ..CsvOptions::default()
            };
            import_csv(&path, &options, 1 << 30, &mut |_| true)
        };
        let (doc, report) = run(OnError::ErrorColumn).unwrap();
        let names: Vec<_> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(names, ["n", "n errors", "m", "m errors", "s"]);
        assert_eq!(cells(&doc)[1][1].as_deref(), Some("x"));
        assert_eq!(report.failures, [("n".to_owned(), 1), ("m".to_owned(), 1)]);
        assert!(report.summary().unwrap().contains("errors"));

        let (doc, report) = run(OnError::Null).unwrap();
        assert_eq!(doc.num_columns(), 3);
        assert!(report.summary().unwrap().contains("null"));

        let err = run(OnError::Fail).unwrap_err().to_string();
        assert!(err.contains("ow 2") && err.contains("\"x\""), "{err}");

        // Nothing failed: no error columns.
        let fine = dir.join("fine.csv");
        std::fs::write(&fine, "n\n1\n2\n").unwrap();
        let (doc, report) =
            import_csv(&fine, &CsvOptions::default(), 1 << 30, &mut |_| true).unwrap();
        assert_eq!(doc.num_columns(), 1);
        assert_eq!(report.summary(), None);

        let preview = preview_csv(
            &path,
            &CsvOptions {
                types,
                ..CsvOptions::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(preview.failures, [1, 1, 0]);
    }

    #[test]
    fn quote_characters() {
        let dir = TempDir::new();
        let path = dir.join("q.csv");
        std::fs::write(&path, "a|b\n'x|y'|\"z\"\n").unwrap();
        let read = |quote| {
            let options = CsvOptions {
                delimiter: b'|',
                quote,
                ..CsvOptions::default()
            };
            cells(
                &import_csv(&path, &options, 1 << 30, &mut |_| true)
                    .unwrap()
                    .0,
            )
        };
        assert_eq!(
            read(Some(b'\'')),
            [[Some("x|y".to_owned()), Some("\"z\"".into())]]
        );
        // Without quoting, quotes are ordinary text.
        std::fs::write(&path, "a|b\n'x'|\"z\"\n").unwrap();
        assert_eq!(read(None), [[Some("'x'".to_owned()), Some("\"z\"".into())]]);
        std::fs::write(&path, "a|b\n'x|y'|\"z\"\n").unwrap();

        let out = dir.join("out.csv");
        let doc = import_csv(
            &path,
            &CsvOptions {
                delimiter: b'|',
                quote: Some(b'\''),
                ..CsvOptions::default()
            },
            1 << 30,
            &mut |_| true,
        )
        .unwrap()
        .0;
        let export_with = |quote| {
            let options = CsvExportOptions {
                delimiter: b'|',
                quote,
                ..CsvExportOptions::default()
            };
            export(
                &doc.output().unwrap(),
                &out,
                &ExportFormat::Csv(options),
                &mut |_| true,
            )
            .unwrap();
            std::fs::read_to_string(&out).unwrap()
        };
        assert_eq!(export_with(Some(b'\'')).lines().nth(1), Some("'x|y'|\"z\""));
        assert_eq!(export_with(None).lines().nth(1), Some("x|y|\"z\""));
    }

    #[test]
    fn large_csv_is_paged_and_cancellable() {
        let dir = TempDir::new();
        let path = dir.join("big.csv");
        let mut text = String::from("n,label\n");
        for i in 0..200_000 {
            text.push_str(&format!("{i},row {i}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let doc = import_csv(&path, &CsvOptions::default(), 1024, &mut |_| true)
            .unwrap()
            .0;
        assert_eq!(doc.source_mode(), crate::SourceMode::Paged);
        assert_eq!(doc.num_rows(), 200_000);
        let last = format_batch(&doc.read(199_999..200_000).unwrap()).unwrap();
        assert_eq!(last[0][1].as_deref(), Some("row 199999"));
        let result = import_csv(&path, &CsvOptions::default(), 1024, &mut |f| f < 0.5);
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn json_lines_and_arrays() {
        let dir = TempDir::new();
        let lines = dir.join("a.ndjson");
        std::fs::write(
            &lines,
            "{\"a\": 1, \"b\": \"x\"}\n{\"a\": 2, \"b\": null, \"c\": {\"d\": [1, 2]}}\n",
        )
        .unwrap();
        let array = dir.join("a.json");
        std::fs::write(
            &array,
            "[\n {\"a\": 1, \"b\": \"x\"},\n {\"a\": 2, \"b\": null, \"c\": {\"d\": [1, 2]}}\n]",
        )
        .unwrap();
        let from_lines = import_json(&lines, 1 << 30, &mut |_| true).unwrap();
        let from_array = import_json(&array, 1 << 30, &mut |_| true).unwrap();
        assert_eq!(from_lines.num_rows(), 2);
        assert_eq!(cells(&from_lines), cells(&from_array));
        assert!(matches!(
            from_lines.schema().field(2).data_type(),
            DataType::Struct(_)
        ));
        assert_eq!(preview_json(&array, 1).unwrap().num_rows(), 1);
        let tricky = dir.join("t.json");
        std::fs::write(&tricky, r#"[{"s": "a]},{\"b"}]"#).unwrap();
        let doc = import_json(&tricky, 1 << 30, &mut |_| true).unwrap();
        assert_eq!(cells(&doc)[0][0].as_deref(), Some("a]},{\"b"));
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "[1, 2]").unwrap();
        assert!(import_json(&bad, 1 << 30, &mut |_| true).is_err());
    }

    #[test]
    fn export_round_trips() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        veta_testkit::fixtures::mixed_settings(&path);
        let doc = Document::open(&path, crate::OpenOptions::default()).unwrap();
        let output = doc.output().unwrap();

        let csv = dir.join("out.csv");
        export(
            &output,
            &csv,
            &ExportFormat::Csv(CsvExportOptions::default()),
            &mut |_| true,
        )
        .unwrap();
        let back = import_csv(&csv, &CsvOptions::default(), 1 << 30, &mut |_| true)
            .unwrap()
            .0;
        assert_eq!(cells(&back), cells(&doc));

        let eu = dir.join("eu.csv");
        let options = CsvExportOptions {
            delimiter: b';',
            decimal: ',',
            ..CsvExportOptions::default()
        };
        export(&output, &eu, &ExportFormat::Csv(options), &mut |_| true).unwrap();
        let text = std::fs::read_to_string(&eu).unwrap();
        assert!(text.lines().nth(2).unwrap().contains(";0,5;"), "{text}");
        let back = import_csv(
            &eu,
            &CsvOptions {
                delimiter: b';',
                decimal: ',',
                ..CsvOptions::default()
            },
            1 << 30,
            &mut |_| true,
        )
        .unwrap()
        .0;
        assert_eq!(cells(&back), cells(&doc));

        for format in [ExportFormat::JsonLines, ExportFormat::JsonArray] {
            let json = dir.join("out.json");
            export(&output, &json, &format, &mut |_| true).unwrap();
            let back = import_json(&json, 1 << 30, &mut |_| true).unwrap();
            assert_eq!(back.num_rows(), 1000);
            assert_eq!(cells(&back)[1], cells(&doc)[1]);
        }
        assert!(matches!(
            export(
                &output,
                &dir.join("x.csv"),
                &ExportFormat::JsonLines,
                &mut |_| false
            ),
            Err(Error::Cancelled)
        ));
        assert!(!dir.join("x.csv").exists());
    }

    #[test]
    fn more_encodings_dates_and_late_json_keys() {
        let dir = TempDir::new();
        // UTF-16 LE with a byte order mark, read with the default (UTF-8).
        let path = dir.join("u16.csv");
        let text = "name,when\nzoë,31/12/2024\n€uro,01/02/2023\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&path, bytes).unwrap();
        let mut options = CsvOptions {
            types: Some(vec![DataType::Utf8, DataType::Date32]),
            date_format: Some("%d/%m/%Y".into()),
            ..CsvOptions::default()
        };
        let (doc, report) = import_csv(&path, &options, usize::MAX, &mut |_| true).unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        let cells = crate::display::format_batch(&doc.read(0..2).unwrap()).unwrap();
        assert_eq!(cells[0][0].as_deref(), Some("zoë"));
        assert_eq!(cells[1][0].as_deref(), Some("€uro"));
        assert_eq!(cells[0][1].as_deref(), Some("2024-12-31"));

        // Windows-1252: 0x80 is €.
        let path = dir.join("w.csv");
        std::fs::write(&path, b"a\n\x80 5\n").unwrap();
        options = CsvOptions {
            encoding: TextEncoding::Windows1252,
            ..CsvOptions::default()
        };
        let (doc, _) = import_csv(&path, &options, usize::MAX, &mut |_| true).unwrap();
        let cells = crate::display::format_batch(&doc.read(0..1).unwrap()).unwrap();
        assert_eq!(cells[0][0].as_deref(), Some("€ 5"));

        // A key that first appears after the inference window is kept.
        let path = dir.join("late.ndjson");
        let mut lines: String = (0..1500).map(|i| format!("{{\"a\": {i}}}\n")).collect();
        lines.push_str("{\"a\": 1, \"late\": \"x\"}\n");
        std::fs::write(&path, lines).unwrap();
        let doc = import_json(&path, usize::MAX, &mut |_| true).unwrap();
        assert_eq!(doc.num_columns(), 2);
        let last = crate::display::format_batch(&doc.read(1500..1501).unwrap()).unwrap();
        assert_eq!(last[0][1].as_deref(), Some("x"));
    }

    #[test]
    fn utf16_split_across_chunks() {
        // A surrogate pair split between two decode calls.
        let text = "a😀b";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let (first, rest) = decode_utf16(&bytes[..4], true, false);
        let mut all = rest;
        all.extend_from_slice(&bytes[4..]);
        let (second, rest) = decode_utf16(&all, true, true);
        assert!(rest.is_empty());
        let joined = String::from_utf8([first, second].concat()).unwrap();
        assert_eq!(joined, text);
    }
}
