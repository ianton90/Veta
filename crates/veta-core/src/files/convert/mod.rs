//! Importing CSV and JSON into documents, and exporting a document's output
//! to CSV or JSON.
//!
//! Imports stream the input in batches into a temporary Parquet file; small
//! results are then loaded into memory, larger ones stay paged from that
//! file (deleted when the document is closed). Exports stream the
//! evaluated output (after all steps) to a temporary file next to the
//! target, renamed into place when complete.

mod chunks;
mod csv;
mod encoding;
mod export;
mod json;
#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ParquetRecordBatchReaderBuilder};

use crate::error::{Error, Result};
use crate::model::Document;
use crate::source::{DataSource, MemorySource};
use crate::steps::checkpoint::{Checkpoint, TempFile};
use crate::steps::{Cast, CastOptions, OnError, Pipeline};

pub use csv::{convert_csv, error_column_name, import_csv, preview_csv};
pub use encoding::TextEncoding;
use encoding::*;
pub use export::export;
pub use json::{import_json, preview_json};

/// Rows per batch when reading or writing.
const BATCH_ROWS: usize = 64 * 1024;
/// Rows read to guess column types.
pub const INFER_ROWS: usize = 1000;

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

/// What an import found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub rows: u64,
    /// Columns with values that didn't fit their type, and how many.
    pub failures: Vec<(String, u64)>,
    /// What happened to them.
    pub on_error: Option<OnError>,
}

// Storing imported batches.

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
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
    crate::steps::checkpoint::write_parquet(&file, schema, batches)?;
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
