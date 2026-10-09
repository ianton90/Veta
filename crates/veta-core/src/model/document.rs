use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use super::{FileInfo, FileMetadata, WriterSettings};
use crate::error::Result;
use crate::history::History;
use crate::io::{OpenOptions, open_parquet};
use crate::source::{MemorySource, SourceMode};
use crate::steps::{Pipeline, Step};

/// One open file (or a new, unsaved one): its source data, the steps applied
/// to it, and file-level settings. See `docs/ARCHITECTURE.md`.
#[derive(Debug)]
pub struct Document {
    pub(crate) path: Option<PathBuf>,
    pub(crate) pipeline: Pipeline,
    pub(crate) info: Option<FileInfo>,
    pub(crate) metadata: FileMetadata,
    pub(crate) writer: WriterSettings,
    pub(crate) history: History,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            path: None,
            pipeline: Pipeline::new(Arc::new(MemorySource::empty())),
            info: None,
            metadata: FileMetadata::default(),
            writer: WriterSettings::default(),
            history: History::new(),
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
            pipeline: Pipeline::new(opened.source),
            info: Some(opened.info),
            metadata: opened.metadata,
            writer: opened.writer,
            history: History::new(),
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

    /// Schema after all steps.
    pub fn schema(&self) -> SchemaRef {
        self.pipeline.schema()
    }

    /// Row count after all steps.
    pub fn num_rows(&self) -> usize {
        self.pipeline.num_rows()
    }

    pub fn num_columns(&self) -> usize {
        self.pipeline.schema().fields().len()
    }

    /// Reads rows in `range` (clamped to the row count), after all steps.
    pub fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        self.pipeline.read(range)
    }

    pub fn steps(&self) -> &[Step] {
        self.pipeline.steps()
    }

    pub fn source_mode(&self) -> SourceMode {
        self.pipeline.source().mode()
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

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Whether there are changes since the document was opened or saved.
    pub fn is_modified(&self) -> bool {
        self.history.is_modified()
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
