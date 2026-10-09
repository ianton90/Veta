use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use super::{FileInfo, FileMetadata, WriterSettings};
use crate::error::Result;
use crate::io::{OpenOptions, open_parquet};
use crate::source::{DataSource, MemorySource, SourceMode};

/// One open file (or a new, unsaved one).
///
/// Steps and history are added by later milestones (see
/// `docs/ARCHITECTURE.md`).
#[derive(Debug)]
pub struct Document {
    pub(crate) path: Option<PathBuf>,
    pub(crate) source: Arc<dyn DataSource>,
    pub(crate) info: Option<FileInfo>,
    pub(crate) metadata: FileMetadata,
    pub(crate) writer: WriterSettings,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            path: None,
            source: Arc::new(MemorySource::empty()),
            info: None,
            metadata: FileMetadata::default(),
            writer: WriterSettings::default(),
        }
    }
}

impl Document {
    /// A new, empty document that has never been saved.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a Parquet file.
    pub fn open(path: impl Into<PathBuf>, options: OpenOptions) -> Result<Self> {
        let path = path.into();
        let opened = open_parquet(&path, options)?;
        Ok(Self {
            path: Some(path),
            source: opened.source,
            info: Some(opened.info),
            metadata: opened.metadata,
            writer: opened.writer,
        })
    }

    /// Where the document is saved, if it has been.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Name to show in tabs and window titles.
    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    pub fn schema(&self) -> SchemaRef {
        self.source.schema()
    }

    pub fn num_rows(&self) -> usize {
        self.source.num_rows()
    }

    pub fn num_columns(&self) -> usize {
        self.source.schema().fields().len()
    }

    /// Reads rows in `range` (clamped to the row count).
    pub fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        self.source.read(range)
    }

    pub fn source_mode(&self) -> SourceMode {
        self.source.mode()
    }

    /// Layout of the file the document was opened from, if any.
    pub fn file_info(&self) -> Option<&FileInfo> {
        self.info.as_ref()
    }

    pub fn metadata(&self) -> &FileMetadata {
        &self.metadata
    }

    pub fn writer_settings(&self) -> &WriterSettings {
        &self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_testkit::{TempDir, fixtures};

    #[test]
    fn title_uses_file_name_or_untitled() {
        let mut doc = Document::new();
        assert_eq!(doc.title(), "Untitled");

        doc.path = Some(PathBuf::from("/data/sales.parquet"));
        assert_eq!(doc.title(), "sales.parquet");
    }

    #[test]
    fn open_reads_rows() {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);

        let doc = Document::open(&path, OpenOptions::default()).unwrap();
        assert_eq!(doc.title(), "mixed.parquet");
        assert_eq!(doc.num_rows(), fixtures::MIXED_SETTINGS_ROWS);
        assert_eq!(doc.num_columns(), 4);
        assert_eq!(doc.read(10..20).unwrap().num_rows(), 10);
        assert!(doc.file_info().is_some());
    }
}
