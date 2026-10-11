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
    #[default]
    Utf8,
    /// ISO-8859-1 / Windows "ANSI" for Western languages.
    Latin1,
}

/// How to read a CSV file.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvOptions {
    pub delimiter: u8,
    pub quote: u8,
    /// The first row holds column names.
    pub header: bool,
    pub encoding: TextEncoding,
    /// Decimal separator in numbers.
    pub decimal: char,
    /// Column types; `None` guesses them from the first rows. Values that
    /// don't fit a column's type become null.
    pub types: Option<Vec<DataType>>,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: b'"',
            header: true,
            encoding: TextEncoding::Utf8,
            decimal: '.',
            types: None,
        }
    }
}

/// The first rows of a CSV file, as they would be imported.
#[derive(Debug, Clone)]
pub struct CsvPreview {
    /// Column names and types (guessed unless given).
    pub schema: SchemaRef,
    pub rows: RecordBatch,
}

/// How to write a CSV file.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvExportOptions {
    pub delimiter: u8,
    pub quote: u8,
    pub header: bool,
    /// Decimal separator for numbers.
    pub decimal: char,
}

impl Default for CsvExportOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: b'"',
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
            return Ok(0);
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
        }
        self.pending = match self.encoding {
            TextEncoding::Utf8 => chunk,
            TextEncoding::Latin1 => chunk
                .iter()
                .map(|&b| b as char)
                .collect::<String>()
                .into_bytes(),
        };
        self.read(buf)
    }
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
        .with_quote(options.quote)
        .with_truncated_rows(true)
}

/// Converts text columns to the wanted types (invalid values become null).
fn convert_columns(batch: &RecordBatch, schema: &SchemaRef, decimal: char) -> Result<RecordBatch> {
    let columns = batch
        .columns()
        .iter()
        .zip(schema.fields())
        .map(|(column, field)| {
            if field.data_type() == &DataType::Utf8 {
                return Ok(column.clone());
            }
            Cast {
                column: field.name().clone(),
                to: field.data_type().clone(),
                options: CastOptions {
                    decimal,
                    thousands: None,
                    format: None,
                    on_error: OnError::Null,
                },
            }
            .convert(column)
        })
        .collect::<Result<Vec<ArrayRef>>>()?;
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
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
    let rows = convert_columns(&text, &schema, options.decimal)?;
    Ok(CsvPreview { schema, rows })
}

/// Imports a CSV file as a new, unsaved document. Results larger than
/// `memory_budget` bytes stay on disk and are paged.
pub fn import_csv(
    path: &Path,
    options: &CsvOptions,
    memory_budget: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Document> {
    let schema = csv_schema(path, options)?;
    let mut input = Input::open(path, options.encoding, Some(progress))?;
    let reader = arrow::csv::ReaderBuilder::new(text_schema(&schema))
        .with_format(csv_format(options))
        .with_batch_size(BATCH_ROWS)
        .build(&mut input)?;
    let batches = reader.map(|batch| convert_columns(&batch?, &schema, options.decimal));
    let source = store(&schema, batches, memory_budget);
    let source = cancelled(source, input.cancelled)?;
    Ok(Document::imported(file_name(path), source))
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

/// Column names and types guessed from a JSON file's first objects, in the
/// order keys first appear.
fn json_schema(path: &Path) -> Result<SchemaRef> {
    let (input, _) = json_input(path, None)?;
    let (schema, _) =
        arrow::json::reader::infer_json_schema(BufReader::new(input), Some(INFER_ROWS))?;
    let (input, _) = json_input(path, None)?;
    let order = key_order(BufReader::new(input), INFER_ROWS);
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
/// new, unsaved document. Types are guessed from the first objects.
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
            let mut writer = arrow::csv::WriterBuilder::new()
                .with_header(options.header)
                .with_delimiter(options.delimiter)
                .with_quote(options.quote)
                .build(file);
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
        let doc = import_csv(&path, &CsvOptions::default(), 1 << 30, &mut |_| true).unwrap();
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
        let doc = import_csv(&path, &options, 1 << 30, &mut |_| true).unwrap();
        let c = cells(&doc);
        assert_eq!(c[0][1].as_deref(), Some("Café"));
        assert_eq!(c[0][2].as_deref(), Some("1.5"));
        assert_eq!(c[1][2], None, "invalid values become null");
        options.types = Some(vec![DataType::Utf8]);
        assert!(import_csv(&path, &options, 1 << 30, &mut |_| true).is_err());
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
        let doc = import_csv(&path, &CsvOptions::default(), 1024, &mut |_| true).unwrap();
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
        let back = import_csv(&csv, &CsvOptions::default(), 1 << 30, &mut |_| true).unwrap();
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
        .unwrap();
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
}
