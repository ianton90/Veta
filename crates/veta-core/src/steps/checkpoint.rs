//! Temporary Parquet files holding a step's materialized output.

use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions};

use crate::error::Result;
use crate::source::{DataSource, PagedSource, SourceMode};

/// Memory budget for reading a checkpoint back.
const CHECKPOINT_BUDGET: usize = 256 * 1024 * 1024;

/// A file in the temp directory, deleted when dropped.
#[derive(Debug)]
pub(super) struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub(super) fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("veta-{}-{n}.parquet", std::process::id()));
        File::create(&path)?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A step's output stored in a temporary Parquet file and read on demand.
#[derive(Debug)]
pub(crate) struct Checkpoint {
    schema: SchemaRef,
    // Declared before `_file` so the file is closed before it is deleted.
    source: PagedSource,
    _file: TempFile,
}

impl Checkpoint {
    /// Opens `file`, whose columns must match `schema`.
    pub(super) fn open(file: TempFile, schema: SchemaRef) -> Result<Self> {
        let handle = File::open(file.path())?;
        let metadata = ArrowReaderMetadata::load(&handle, ArrowReaderOptions::new())?;
        Ok(Self {
            schema,
            source: PagedSource::new(handle, metadata, CHECKPOINT_BUDGET),
            _file: file,
        })
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &Path {
        self._file.path()
    }
}

impl DataSource for Checkpoint {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.source.num_rows()
    }

    fn mode(&self) -> SourceMode {
        SourceMode::Paged
    }

    fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        let batch = self.source.read(range)?;
        // Same columns; the schema read back may differ in metadata.
        Ok(RecordBatch::try_new(
            self.schema.clone(),
            batch.columns().to_vec(),
        )?)
    }
}
