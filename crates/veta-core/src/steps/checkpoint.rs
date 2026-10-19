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

/// A file in the temp directory, deleted when dropped.
#[derive(Debug)]
pub(crate) struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub(crate) fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("veta-{}-{n}.parquet", std::process::id()));
        File::create(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Deletes temporary files left in the temp directory by Veta processes
/// that ended without cleaning up (a crash). Returns how many were removed.
///
/// A file counts as stale when its process is gone (Linux), or when it is
/// over a day old (other Unix systems; deleting a file another process has
/// open is harmless there), or when nothing has it open and it is over an
/// hour old (Windows).
pub fn remove_stale_temp_files() -> usize {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return 0;
    };
    let own = std::process::id();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("veta-"))
            .and_then(|n| n.strip_suffix(".parquet"))
            .and_then(|n| n.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != own && is_stale(&entry.path(), pid) && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn age(path: &Path) -> std::time::Duration {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn is_stale(_path: &Path, pid: u32) -> bool {
    !Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_stale(path: &Path, _pid: u32) -> bool {
    age(path) > std::time::Duration::from_secs(24 * 3600)
}

#[cfg(windows)]
fn is_stale(path: &Path, _pid: u32) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    // Opening without sharing fails while another process has it open.
    let unused = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .is_ok();
    unused && age(path) > std::time::Duration::from_secs(3600)
}

#[cfg(not(any(unix, windows)))]
fn is_stale(path: &Path, _pid: u32) -> bool {
    age(path) > std::time::Duration::from_secs(24 * 3600)
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
    pub(crate) fn open(file: TempFile, schema: SchemaRef) -> Result<Self> {
        let handle = File::open(file.path())?;
        let metadata = ArrowReaderMetadata::load(&handle, ArrowReaderOptions::new())?;
        Ok(Self {
            schema,
            source: PagedSource::new(
                handle,
                metadata,
                crate::memory::page_cache(crate::memory::budget()),
            )
            .with_path(file.path()),
            _file: file,
        })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
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

/// Writes `batches` (with the columns of `schema`) to `file`. Temporary
/// files are read back once or a few times: no statistics, and fast LZ4
/// compression (cheaper than writing and reading more bytes on slow disks).
pub(crate) fn write_parquet(
    file: &TempFile,
    schema: &SchemaRef,
    batches: impl Iterator<Item = Result<RecordBatch>>,
) -> Result<()> {
    let props = parquet::file::properties::WriterProperties::builder()
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::None)
        .set_compression(parquet::basic::Compression::LZ4_RAW)
        .build();
    let mut writer = crate::files::writer::ParallelWriter::new(
        File::create(file.path())?,
        schema.clone(),
        props,
    )?;
    for batch in batches {
        let batch = batch?;
        // Concatenated batches may carry different schema metadata.
        let batch = RecordBatch::try_new(schema.clone(), batch.columns().to_vec())?;
        writer.write(&batch)?;
    }
    writer.close()?;
    Ok(())
}

/// Writes `batches` (with the columns of `schema`) to `file` on this
/// thread, with the settings of [`write_parquet`]: for many small files
/// written at the same time (parts of a sort's buckets).
pub(crate) fn write_part(
    file: &TempFile,
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<()> {
    let props = parquet::file::properties::WriterProperties::builder()
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::None)
        .set_compression(parquet::basic::Compression::LZ4_RAW)
        .build();
    let mut writer = parquet::arrow::ArrowWriter::try_new(
        File::create(file.path())?,
        schema.clone(),
        Some(props),
    )?;
    for batch in batches {
        writer.write(&RecordBatch::try_new(
            schema.clone(),
            batch.columns().to_vec(),
        )?)?;
    }
    writer.close()?;
    Ok(())
}
