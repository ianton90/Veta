//! Writing a frame to a file. Each file is written to a temporary file
//! next to it, renamed into place when complete.

use std::path::Path;

use super::DataFrame;
use crate::error::Result;
use crate::files::convert::{CsvExportOptions, ExportFormat, write_export};
use crate::files::parquet::write_batches;
use crate::files::replace::Replacement;
use crate::model::WriterSettings;
use crate::plan::Task;

impl DataFrame {
    /// Writes a Parquet file, with the writer settings of the file read
    /// last (the defaults for other sources).
    pub fn write_parquet(&self, path: impl AsRef<Path>) -> Result<()> {
        self.write_parquet_with(path, &self.node.file.writer.clone())
    }

    /// Like [`Self::write_parquet`], with these settings.
    pub fn write_parquet_with(
        &self,
        path: impl AsRef<Path>,
        settings: &WriterSettings,
    ) -> Result<()> {
        let (replacement, file) = Replacement::create(path.as_ref())?;
        let batches = self
            .node
            .read_parallel(None, self.reporting(Task::Writing), |batches, _| {
                Ok(batches)
            })
            .flat_map(|piece| match piece {
                Ok(batches) => batches.into_iter().map(Ok).collect(),
                Err(e) => vec![Err(e)],
            });
        write_batches(
            &self.schema(),
            batches,
            &self.node.file.metadata,
            settings,
            file,
        )?;
        replacement.commit()
    }

    /// Writes a CSV file.
    pub fn write_csv(&self, path: impl AsRef<Path>) -> Result<()> {
        self.write_csv_with(path, CsvExportOptions::default())
    }

    /// Like [`Self::write_csv`], with options.
    pub fn write_csv_with(&self, path: impl AsRef<Path>, options: CsvExportOptions) -> Result<()> {
        self.export(path.as_ref(), ExportFormat::Csv(options))
    }

    /// Writes a JSON file: an array of objects.
    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<()> {
        self.export(path.as_ref(), ExportFormat::JsonArray)
    }

    /// Writes a JSON lines file: one object per line.
    pub fn write_ndjson(&self, path: impl AsRef<Path>) -> Result<()> {
        self.export(path.as_ref(), ExportFormat::JsonLines)
    }

    fn export(&self, path: &Path, format: ExportFormat) -> Result<()> {
        let (replacement, file) = Replacement::create(path)?;
        if let Some(progress) = &self.progress {
            progress.start(Task::Writing, 0);
        }
        let mut report = |fraction| match &self.progress {
            Some(progress) => progress.set_fraction(fraction),
            None => true,
        };
        write_export(&self.node, file, &format, &mut report)?;
        replacement.commit()
    }
}
