use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use super::{FileInfo, FileMetadata, WriterSettings};
use crate::error::Error;
use crate::error::Result;
use crate::history::History;
use crate::io::{OpenOptions, SaveJob, SaveResult, open_parquet, same_file};
use crate::source::{MemorySource, SourceMode};
use crate::steps::{Pipeline, Step};

/// One open file (or a new, unsaved one): its source data, the steps applied
/// to it, and file-level settings. See `docs/ARCHITECTURE.md`.
#[derive(Debug)]
pub struct Document {
    /// Where the document is saved (the file it was opened from, until it
    /// is saved elsewhere).
    pub(crate) path: Option<PathBuf>,
    /// The file the source data is read from, and how it was opened.
    pub(crate) source_file: Option<(PathBuf, OpenOptions)>,
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
            source_file: None,
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
            source_file: Some((path.clone(), options)),
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

    /// Prepares writing the document to `target`, or to its own path when
    /// `None`. Run the job (possibly on another thread), then pass its
    /// result to [`Document::finish_save`].
    pub fn save_job(&self, target: Option<PathBuf>) -> Result<SaveJob> {
        let target = match target.or_else(|| self.path.clone()) {
            Some(t) => t,
            None => {
                return Err(Error::InvalidCommand(
                    "choose where to save the file".into(),
                ));
            }
        };
        if self.num_columns() == 0 {
            return Err(Error::InvalidCommand(
                "a file needs at least one column".into(),
            ));
        }
        let reopen = self
            .source_file
            .as_ref()
            .filter(|(source, _)| same_file(source, &target))
            .map(|(_, options)| *options);
        Ok(SaveJob {
            pipeline: self.pipeline.clone(),
            metadata: self.metadata.clone(),
            writer: self.writer.clone(),
            target,
            reopen,
        })
    }

    /// Updates the document after its save job succeeded. Saving over the
    /// source file reloads it, which also clears the steps and undo history
    /// (they are now part of the file).
    pub fn finish_save(&mut self, result: SaveResult) {
        if let Some(opened) = result.reopened {
            self.pipeline = Pipeline::new(opened.source);
            self.info = Some(opened.info);
            self.metadata = opened.metadata;
            self.writer = opened.writer;
            self.history = History::new();
            if let Some((source, _)) = &mut self.source_file {
                *source = result.target.clone();
            }
        } else {
            self.history.mark_saved();
        }
        self.path = Some(result.target);
    }

    /// Saves on the current thread. See [`Document::save_job`].
    pub fn save(&mut self, target: Option<PathBuf>) -> Result<()> {
        let result = self.save_job(target)?.run()?;
        self.finish_save(result);
        Ok(())
    }

    /// Whether there are changes since the document was opened or saved.
    pub fn is_modified(&self) -> bool {
        self.history.is_modified()
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Row ranges.
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

    use crate::controller::{execute, undo};
    use crate::{Command, StatisticsLevel};

    fn reopen(path: &Path) -> Document {
        Document::open(path, OpenOptions::default()).unwrap()
    }

    #[test]
    fn save_as_keeps_writer_settings_and_metadata() {
        let dir = TempDir::new();
        let source = dir.join("mixed.parquet");
        fixtures::mixed_settings(&source);
        let original = reopen(&source);

        let mut doc = reopen(&source);
        let copy = dir.join("copy.parquet");
        doc.save(Some(copy.clone())).unwrap();
        assert_eq!(doc.path(), Some(copy.as_path()));
        assert!(!doc.is_modified());

        let saved = reopen(&copy);
        assert_eq!(saved.writer_settings(), original.writer_settings());
        let info = saved.file_info().unwrap();
        assert_eq!(
            info.row_groups.len(),
            original.file_info().unwrap().row_groups.len()
        );
        for (key, value) in fixtures::MIXED_SETTINGS_METADATA {
            assert_eq!(
                saved.metadata().get(key).unwrap().value.as_deref(),
                Some(*value)
            );
        }
        assert_eq!(
            saved.read(0..1000).unwrap(),
            original.read(0..1000).unwrap()
        );
        assert!(
            dir.path()
                .read_dir()
                .unwrap()
                .all(|e| { !e.unwrap().file_name().to_string_lossy().ends_with(".tmp") })
        );
    }

    #[test]
    fn save_as_keeps_steps_and_undo() {
        let dir = TempDir::new();
        let source = dir.join("mixed.parquet");
        fixtures::mixed_settings(&source);
        let mut doc = reopen(&source);
        execute(&mut doc, Command::DeleteRows { rows: vec![0..10] }).unwrap();
        doc.save(Some(dir.join("out.parquet"))).unwrap();
        assert_eq!(doc.steps().len(), 1, "source untouched, steps kept");
        assert!(!doc.is_modified());
        assert_eq!(reopen(&dir.join("out.parquet")).num_rows(), 990);
        assert_eq!(reopen(&source).num_rows(), 1000);
        undo(&mut doc);
        assert!(doc.is_modified());
    }

    #[test]
    fn save_in_place_applies_edits_and_reloads() {
        for options in [OpenOptions::default(), OpenOptions::paged()] {
            let dir = TempDir::new();
            let path = dir.join("mixed.parquet");
            fixtures::mixed_settings(&path);
            let mut doc = Document::open(&path, options).unwrap();
            let rows = doc.num_rows();
            execute(&mut doc, Command::InsertRows { at: 0, count: 1 }).unwrap();
            execute(
                &mut doc,
                Command::SetCell {
                    row: 0,
                    column: "name".into(),
                    value: Some("new".into()),
                },
            )
            .unwrap();
            execute(
                &mut doc,
                Command::RenameColumn {
                    from: "score".into(),
                    to: "points".into(),
                },
            )
            .unwrap();
            doc.save(None).unwrap();

            assert!(doc.steps().is_empty());
            assert!(!doc.history().can_undo());
            assert!(!doc.is_modified());
            assert_eq!(doc.num_rows(), rows + 1);
            let reloaded = reopen(&path);
            assert_eq!(reloaded.num_rows(), rows + 1);
            assert!(reloaded.schema().field_with_name("points").is_ok());
            // The renamed column kept its settings (gzip, no statistics).
            let points = reloaded.writer_settings().column("points");
            assert_eq!(points.statistics, StatisticsLevel::None);
            assert!(matches!(points.compression, crate::Compression::Gzip(_)));
        }
    }

    #[test]
    fn failed_save_leaves_everything_alone() {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);
        let mut doc = reopen(&path);
        execute(&mut doc, Command::InsertRows { at: 0, count: 1 }).unwrap();
        let err = doc.save(Some(dir.join("missing-dir").join("x.parquet")));
        assert!(err.is_err());
        assert!(doc.is_modified());
        assert_eq!(doc.path(), Some(path.as_path()));
        assert!(Document::new().save(None).is_err(), "no path");
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
