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

/// A file in Veta's temp folder ([`temp_dir`]), deleted when dropped.
#[derive(Debug)]
pub(crate) struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub(crate) fn new() -> Result<Self> {
        Self::with_extension("parquet")
    }

    /// A temp file named `….{extension}` (`parquet` or `arrow`).
    pub(crate) fn with_extension(extension: &str) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = temp_dir()?.join(format!("veta-{}-{n}.{extension}", std::process::id()));
        // A new file only (never one already there), readable by its owner
        // alone.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&path)?;
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

/// Veta's temp folder, only the user can open (#161; decision 61). Linux:
/// `$XDG_CACHE_HOME/veta/tmp` (default `~/.cache/veta/tmp`): per-user and on
/// disk, while `/tmp` is shared and, like `/run/user`, often kept in memory,
/// too small for files of several GB. Elsewhere the system temp folder is
/// already per-user: a `veta` folder inside it.
pub fn temp_dir() -> Result<PathBuf> {
    let dir = temp_dir_path();
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir)?;
    // A folder made by someone else (or a link to one) isn't private.
    let meta = std::fs::symlink_metadata(&dir)?;
    #[cfg(target_os = "linux")]
    let theirs = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").is_ok_and(|me| me.uid() != meta.uid())
    };
    #[cfg(not(target_os = "linux"))]
    let theirs = false;
    if !meta.is_dir() || theirs {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a private folder", dir.display()),
        )
        .into());
    }
    // Made earlier with other permissions: private again.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(dir)
}

fn temp_dir_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .and_then(absolute)
            .or_else(|| {
                std::env::var_os("HOME")
                    .and_then(absolute)
                    .map(|h| h.join(".cache"))
            });
        if let Some(cache) = cache {
            return cache.join("veta").join("tmp");
        }
    }
    std::env::temp_dir().join("veta")
}

/// Deletes temporary files left by Veta processes that ended without
/// cleaning up (a crash), in Veta's temp folder and where versions before
/// 0.7 kept them (the system temp folder). Returns how many were removed.
///
/// A file counts as stale when its process is gone (Linux), or when it is
/// over a day old (other Unix systems; deleting a file another process has
/// open is harmless there), or when nothing has it open and it is over an
/// hour old (Windows).
pub fn remove_stale_temp_files() -> usize {
    let dirs = [temp_dir().ok(), Some(std::env::temp_dir())];
    dirs.into_iter()
        .flatten()
        .map(|dir| remove_stale_in(&dir))
        .sum()
}

fn remove_stale_in(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let own = std::process::id();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("veta-"))
            .and_then(|n| {
                n.strip_suffix(".parquet")
                    .or_else(|| n.strip_suffix(".arrow"))
            })
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
