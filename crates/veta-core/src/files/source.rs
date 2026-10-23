//! Data sources: where a document's original rows come from.
//!
//! [`MemorySource`] holds everything in memory. [`PagedSource`] reads a
//! Parquet file in chunks on demand and keeps recently used chunks in a cache
//! bounded by a memory budget.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::ops::Range;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use arrow::compute::concat_batches;
use arrow::datatypes::{Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ParquetRecordBatchReaderBuilder, RowSelection, RowSelector,
};

use crate::error::{Error, Result};

/// How a source holds its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    InMemory,
    Paged,
}

/// Read access to a table of rows.
pub trait DataSource: fmt::Debug + Send + Sync {
    fn schema(&self) -> SchemaRef;

    fn num_rows(&self) -> usize;

    fn mode(&self) -> SourceMode;

    /// Reads rows in `range` (clamped to the number of rows) as one batch.
    fn read(&self, range: Range<usize>) -> Result<RecordBatch>;

    /// Like [`Self::read`], with only the columns named in `names` (in the
    /// schema's order). Sources that store columns apart read only those.
    fn read_columns(&self, range: Range<usize>, names: &[String]) -> Result<RecordBatch> {
        project(&self.read(range)?, names)
    }
}

/// The columns of `batch` named in `names`, in the batch's order.
pub(crate) fn project(batch: &RecordBatch, names: &[String]) -> Result<RecordBatch> {
    let indices: Vec<usize> = batch
        .schema()
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| names.contains(f.name()))
        .map(|(i, _)| i)
        .collect();
    Ok(batch.project(&indices)?)
}

/// A source holding all its rows in memory.
#[derive(Debug)]
pub struct MemorySource {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
    /// Row index at which each batch starts, plus the total row count.
    offsets: Vec<usize>,
}

impl MemorySource {
    /// Batches must all have the fields of `schema` (schema-level metadata may
    /// differ).
    pub fn new(schema: SchemaRef, batches: Vec<RecordBatch>) -> Result<Self> {
        if let Some(bad) = batches
            .iter()
            .find(|b| b.schema().fields() != schema.fields())
        {
            return Err(Error::from(arrow::error::ArrowError::SchemaError(format!(
                "batch schema {:?} does not match source schema",
                bad.schema()
            ))));
        }
        let batches: Vec<_> = batches.into_iter().filter(|b| b.num_rows() > 0).collect();
        let mut offsets = Vec::with_capacity(batches.len() + 1);
        let mut total = 0;
        for batch in &batches {
            offsets.push(total);
            total += batch.num_rows();
        }
        offsets.push(total);
        Ok(Self {
            schema,
            batches,
            offsets,
        })
    }

    /// A source with no columns and no rows.
    pub fn empty() -> Self {
        Self {
            schema: Arc::new(Schema::empty()),
            batches: Vec::new(),
            offsets: vec![0],
        }
    }
}

impl DataSource for MemorySource {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0)
    }

    fn mode(&self) -> SourceMode {
        SourceMode::InMemory
    }

    fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        let range = clamp(range, self.num_rows());
        let mut parts = Vec::new();
        for (i, batch) in self.batches.iter().enumerate() {
            let (start, end) = (self.offsets[i], self.offsets[i + 1]);
            if end <= range.start {
                continue;
            }
            if start >= range.end {
                break;
            }
            let from = range.start.max(start) - start;
            let to = range.end.min(end) - start;
            parts.push(batch.slice(from, to - from));
        }
        Ok(concat_batches(&self.schema, &parts)?)
    }
}

/// Rows per cached chunk in [`PagedSource`]. Chunks never cross row groups.
pub const CHUNK_ROWS: usize = 64 * 1024;

/// A Parquet file read on demand.
///
/// The file is split into chunks of at most [`CHUNK_ROWS`] rows within each
/// row group. Chunks are decoded when first read and kept in an LRU cache
/// whose decoded size stays under the memory budget; at least one chunk is
/// always kept so reads larger than the budget still succeed.
pub struct PagedSource {
    file: File,
    metadata: ArrowReaderMetadata,
    schema: SchemaRef,
    chunks: Chunks,
    num_rows: usize,
    budget: usize,
    cache: Mutex<ChunkCache>,
    /// The file's path, with its size and modification time when opened:
    /// extra handles on it let several readers decode at the same time.
    path: Option<(std::path::PathBuf, u64, Option<std::time::SystemTime>)>,
    /// Readers of recently decoded chunks, kept to decode the next chunk of
    /// the same row group: reading chunks in order (a full pass, saving)
    /// then decompresses each page and dictionary once instead of once per
    /// chunk. Several threads reading different parts of the file each
    /// continue their own.
    cursors: Mutex<Vec<Cursor>>,
    /// Chunks being decoded right now, to share the cores between them.
    decoding: std::sync::atomic::AtomicUsize,
    /// Held while decoding through the source's own handle (no extra
    /// handles can be opened): its readers share one file position.
    shared: Mutex<()>,
}

/// Row group readers positioned at chunk `next`, for these columns. The
/// columns are split into groups of about the same size on disk, one per
/// core, each with its own reader on its own thread, so a chunk's columns
/// decode at the same time, and each reader decodes the next chunk while
/// the last one is used.
struct Cursor {
    next: usize,
    columns: Option<Vec<usize>>,
    /// The columns each reader decodes (in the schema's order), and its
    /// decoded chunks.
    readers: Vec<(Vec<usize>, Receiver<arrow::error::Result<RecordBatch>>)>,
    /// The columns of a chunk, in the schema's order, and its schema.
    order: Vec<usize>,
    schema: SchemaRef,
    /// The reader thread when it reads the source's own handle, joined
    /// when the cursor is dropped.
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for Cursor {
    fn drop(&mut self) {
        // Stops the readers: their next send fails.
        self.readers.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Cursor {
    /// The next chunk, every reader decoding its columns at the same time;
    /// `None` at the end of the row group.
    fn next_batch(&mut self) -> Option<Result<RecordBatch>> {
        let parts: Vec<_> = self.readers.iter().map(|(_, r)| r.recv().ok()).collect();
        let mut columns: HashMap<usize, arrow::array::ArrayRef> = HashMap::new();
        let mut rows = 0;
        for ((group, _), part) in self.readers.iter().zip(parts) {
            let batch = match part? {
                Ok(batch) => batch,
                Err(e) => return Some(Err(e.into())),
            };
            rows = batch.num_rows();
            for (k, &c) in group.iter().enumerate() {
                columns.insert(c, batch.column(k).clone());
            }
        }
        let arrays = self
            .order
            .iter()
            .map(|c| columns.remove(c).expect("every column is read"))
            .collect();
        let options = arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(rows));
        Some(
            RecordBatch::try_new_with_options(self.schema.clone(), arrays, &options)
                .map_err(Error::from),
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ChunkSpan {
    row_group: usize,
    /// First row of the chunk within its row group.
    offset_in_group: usize,
    /// First row of the chunk within the file.
    start: usize,
    len: usize,
}

/// How many chunks row groups of these sizes make.
#[cfg(test)]
pub(crate) fn chunk_count(row_counts: &[usize]) -> usize {
    Chunks::new(row_counts.iter().copied()).len()
}

/// The file's chunks, worked out from its row groups' sizes: a list of every
/// chunk would grow with the row count a file claims (#157).
#[derive(Debug)]
struct Chunks {
    /// Row groups with rows, in order.
    groups: Vec<GroupSpan>,
    rows: usize,
    count: usize,
}

#[derive(Debug, Clone, Copy)]
struct GroupSpan {
    row_group: usize,
    /// First row of the row group within the file.
    start: usize,
    rows: usize,
    /// Index of the row group's first chunk.
    first_chunk: usize,
}

impl Chunks {
    /// From each row group's row count, in order.
    fn new(row_counts: impl Iterator<Item = usize>) -> Self {
        let (mut groups, mut rows, mut count) = (Vec::new(), 0usize, 0usize);
        for (row_group, n) in row_counts.enumerate().filter(|(_, n)| *n > 0) {
            groups.push(GroupSpan {
                row_group,
                start: rows,
                rows: n,
                first_chunk: count,
            });
            rows = rows.saturating_add(n);
            count = count.saturating_add(n.div_ceil(CHUNK_ROWS));
        }
        Self {
            groups,
            rows,
            count,
        }
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn len(&self) -> usize {
        self.count
    }

    /// Chunk `index` (below [`Self::len`]).
    fn get(&self, index: usize) -> ChunkSpan {
        let g = self.groups[self.groups.partition_point(|g| g.first_chunk <= index) - 1];
        let offset = (index - g.first_chunk) * CHUNK_ROWS;
        ChunkSpan {
            row_group: g.row_group,
            offset_in_group: offset,
            start: g.start + offset,
            len: CHUNK_ROWS.min(g.rows - offset),
        }
    }

    /// The chunks holding rows of `range` (within the file), with their
    /// indices.
    fn covering(&self, range: Range<usize>) -> impl Iterator<Item = (usize, ChunkSpan)> + '_ {
        let first = if range.is_empty() {
            self.count
        } else {
            let g = self.groups[self.groups.partition_point(|g| g.start <= range.start) - 1];
            g.first_chunk + (range.start - g.start) / CHUNK_ROWS
        };
        (first..self.count)
            .map(|index| (index, self.get(index)))
            .take_while(move |(_, span)| span.start < range.end)
    }
}

#[derive(Debug, Default)]
struct ChunkCache {
    batches: HashMap<usize, RecordBatch>,
    /// Least recently used first.
    order: VecDeque<usize>,
    bytes: usize,
}

impl PagedSource {
    /// `metadata` must have been loaded from `file`.
    pub fn new(file: File, metadata: ArrowReaderMetadata, budget: usize) -> Self {
        let chunks = Chunks::new(
            metadata
                .metadata()
                .row_groups()
                .iter()
                .map(|rg| usize::try_from(rg.num_rows()).unwrap_or(0)),
        );
        Self {
            schema: metadata.schema().clone(),
            file,
            metadata,
            num_rows: chunks.rows(),
            chunks,
            budget,
            cache: Mutex::new(ChunkCache::default()),
            cursors: Mutex::new(Vec::new()),
            decoding: std::sync::atomic::AtomicUsize::new(0),
            shared: Mutex::new(()),
            path: None,
        }
    }

    /// Lets the source open more handles on the file at `path` (the file it
    /// was given), to decode columns on several cores. A file changed since
    /// is read through the one handle.
    pub fn with_path(mut self, path: &std::path::Path) -> Self {
        if let Ok(meta) = self.file.metadata() {
            self.path = Some((path.to_path_buf(), meta.len(), meta.modified().ok()));
        }
        self
    }

    /// A new handle on the file, if it can be opened and is unchanged.
    fn reopen(&self) -> Option<File> {
        let (path, len, modified) = self.path.as_ref()?;
        let file = File::open(path).ok()?;
        let meta = file.metadata().ok()?;
        (meta.len() == *len && meta.modified().ok() == *modified).then_some(file)
    }

    /// Decoded bytes currently held in the cache.
    pub fn cached_bytes(&self) -> usize {
        self.lock_cache().bytes
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, ChunkCache> {
        // A panic while holding the lock leaves the cache consistent enough to
        // keep using: worst case it holds a stale LRU order.
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn chunk(&self, index: usize) -> Result<RecordBatch> {
        {
            let mut cache = self.lock_cache();
            if let Some(batch) = cache.batches.get(&index).cloned() {
                cache.order.retain(|&i| i != index);
                cache.order.push_back(index);
                return Ok(batch);
            }
        }

        let batch = self.load_chunk(index)?;

        let mut cache = self.lock_cache();
        let size = batch.get_array_memory_size();
        while cache.bytes + size > self.budget {
            let Some(evict) = cache.order.pop_front() else {
                break;
            };
            if let Some(old) = cache.batches.remove(&evict) {
                cache.bytes -= old.get_array_memory_size();
            }
        }
        if cache.batches.insert(index, batch.clone()).is_none() {
            cache.bytes += size;
            cache.order.push_back(index);
        }
        Ok(batch)
    }

    fn load_chunk(&self, index: usize) -> Result<RecordBatch> {
        crate::profile::time("read: decode chunk", || {
            self.load_chunk_columns(index, None)
        })
    }

    /// Decodes chunk `index`, only the top-level columns `columns` if given
    /// (in the schema's order). Continues the last chunk's reader when this
    /// chunk follows it in the same row group.
    fn load_chunk_columns(&self, index: usize, columns: Option<&[usize]>) -> Result<RecordBatch> {
        let span = self.chunks.get(index);
        if columns.is_some_and(<[usize]>::is_empty) {
            let options =
                arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(span.len));
            let schema = Arc::new(Schema::empty());
            return Ok(RecordBatch::try_new_with_options(schema, vec![], &options)?);
        }
        use std::sync::atomic::Ordering::Relaxed;
        self.decoding.fetch_add(1, Relaxed);
        let result = self.decode_chunk(index, span, columns);
        self.decoding.fetch_sub(1, Relaxed);
        result
    }

    fn decode_chunk(
        &self,
        index: usize,
        span: ChunkSpan,
        columns: Option<&[usize]>,
    ) -> Result<RecordBatch> {
        // Without extra handles every reader shares the source's handle:
        // one at a time.
        let _shared = self
            .path
            .is_none()
            .then(|| self.shared.lock().unwrap_or_else(|e| e.into_inner()));
        let found = {
            let mut cursors = self.lock_cursors();
            cursors
                .iter()
                .position(|c| c.next == index && c.columns.as_deref() == columns)
                .map(|i| cursors.remove(i))
        };
        if let Some(mut c) = found
            && let Some(batch) = c.next_batch()
        {
            let batch = batch?;
            if batch.num_rows() == span.len {
                c.next = index + 1;
                self.keep(c);
                return Ok(batch);
            }
        }
        // New readers from this chunk to the end of its row group, giving
        // one chunk per batch.
        let group_rows = usize::try_from(
            self.metadata
                .metadata()
                .row_group(span.row_group)
                .num_rows(),
        )
        .unwrap_or(0);
        let mut selectors = Vec::with_capacity(2);
        if span.offset_in_group > 0 {
            selectors.push(RowSelector::skip(span.offset_in_group));
        }
        selectors.push(RowSelector::select(group_rows - span.offset_in_group));
        let order: Vec<usize> = match columns {
            Some(columns) => columns.to_vec(),
            None => (0..self.schema.fields().len()).collect(),
        };
        let descr = self.metadata.metadata().file_metadata().schema_descr();
        // One handle each: handles share nothing, so readers (also those of
        // other cursors, still finishing a chunk) can't disturb each other's
        // position. Without extra handles, one reader on the source's own
        // handle, after the readers on it have stopped.
        let mut groups = self.column_groups(span.row_group, &order);
        let mut handles: Vec<File> = (0..groups.len()).map_while(|_| self.reopen()).collect();
        let shared = handles.len() < groups.len();
        if shared {
            self.lock_cursors().clear();
            groups = vec![order.clone()];
            handles = vec![self.file.try_clone()?];
        }
        let mut readers = Vec::new();
        let mut threads = Vec::new();
        for (group, handle) in groups.into_iter().zip(handles) {
            let mask = parquet::arrow::ProjectionMask::roots(descr, group.iter().copied());
            let reader =
                ParquetRecordBatchReaderBuilder::new_with_metadata(handle, self.metadata.clone())
                    .with_row_groups(vec![span.row_group])
                    .with_row_selection(RowSelection::from(selectors.clone()))
                    .with_batch_size(CHUNK_ROWS)
                    .with_projection(mask)
                    .build()?;
            // Decodes one chunk ahead; stops when the cursor is dropped.
            let (send, receive) = std::sync::mpsc::sync_channel(0);
            let thread = std::thread::spawn(move || {
                for batch in reader {
                    if send.send(batch).is_err() {
                        break;
                    }
                }
            });
            readers.push((group, receive));
            if shared {
                threads.push(thread);
            }
        }
        let schema = match columns {
            Some(columns) => Arc::new(self.schema.project(columns)?),
            None => self.schema.clone(),
        };
        let mut next = Cursor {
            next: index + 1,
            columns: columns.map(<[usize]>::to_vec),
            readers,
            order,
            schema: schema.clone(),
            threads,
        };
        let batch = match next.next_batch() {
            Some(batch) => batch?,
            None => RecordBatch::new_empty(schema),
        };
        self.keep(next);
        Ok(batch)
    }

    fn lock_cursors(&self) -> std::sync::MutexGuard<'_, Vec<Cursor>> {
        self.cursors.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps `cursor` for the next chunk, dropping the oldest beyond two
    /// per core.
    fn keep(&self, cursor: Cursor) {
        let dropped = {
            let mut cursors = self.lock_cursors();
            cursors.push(cursor);
            let limit = 2 * crate::parallel::threads();
            let extra = cursors.len().saturating_sub(limit);
            cursors.drain(..extra).collect::<Vec<_>>()
        };
        // Outside the lock: dropping may wait for a reader thread.
        drop(dropped);
    }

    /// `columns` (top-level, in the schema's order) split into one group per
    /// core, of about the same compressed size in row group `row_group`.
    fn column_groups(&self, row_group: usize, columns: &[usize]) -> Vec<Vec<usize>> {
        // The cores are shared among the chunks being decoded at once.
        let decoding = self
            .decoding
            .load(std::sync::atomic::Ordering::Relaxed)
            .max(1);
        let count = (crate::parallel::threads() / decoding)
            .min(columns.len())
            .max(1);
        let metadata = self.metadata.metadata();
        let descr = metadata.file_metadata().schema_descr();
        let group = metadata.row_group(row_group);
        let mut sizes = vec![0i64; self.schema.fields().len()];
        for (leaf, chunk) in group.columns().iter().enumerate() {
            if let Some(size) = sizes.get_mut(descr.get_column_root_idx(leaf)) {
                *size += chunk.compressed_size();
            }
        }
        // Largest first, each to the lightest group.
        let mut by_size = columns.to_vec();
        by_size.sort_by_key(|&c| std::cmp::Reverse(sizes.get(c).copied().unwrap_or(0)));
        let mut groups: Vec<(i64, Vec<usize>)> = vec![(0, Vec::new()); count];
        for c in by_size {
            let lightest = groups
                .iter_mut()
                .min_by_key(|(load, _)| *load)
                .expect("at least one group");
            lightest.0 += sizes.get(c).copied().unwrap_or(0);
            lightest.1.push(c);
        }
        groups
            .into_iter()
            .filter(|(_, g)| !g.is_empty())
            .map(|(_, mut g)| {
                g.sort_unstable();
                g
            })
            .collect()
    }
}

impl fmt::Debug for PagedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PagedSource")
            .field("num_rows", &self.num_rows)
            .field("chunks", &self.chunks.len())
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl DataSource for PagedSource {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.num_rows
    }

    fn mode(&self) -> SourceMode {
        SourceMode::Paged
    }

    fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        let range = clamp(range, self.num_rows);
        let mut parts = Vec::new();
        for (index, span) in self.chunks.covering(range.clone()) {
            let batch = self.chunk(index)?;
            let from = range.start.max(span.start) - span.start;
            let to = range.end.min(span.start + span.len) - span.start;
            parts.push(batch.slice(from, to - from));
        }
        Ok(concat_batches(&self.schema, &parts)?)
    }

    fn read_columns(&self, range: Range<usize>, names: &[String]) -> Result<RecordBatch> {
        let range = clamp(range, self.num_rows);
        let columns: Vec<usize> = self
            .schema
            .fields()
            .iter()
            .enumerate()
            .filter(|(_, f)| names.contains(f.name()))
            .map(|(i, _)| i)
            .collect();
        let schema = Arc::new(self.schema.project(&columns)?);
        let mut parts = Vec::new();
        for (index, span) in self.chunks.covering(range.clone()) {
            // A chunk already decoded in full is used; others are decoded
            // with only these columns, and not kept.
            let cached = self.lock_cache().batches.get(&index).cloned();
            let batch = match cached {
                Some(batch) => batch.project(&columns)?,
                None => self.load_chunk_columns(index, Some(&columns))?,
            };
            let from = range.start.max(span.start) - span.start;
            let to = range.end.min(span.start + span.len) - span.start;
            parts.push(batch.slice(from, to - from));
        }
        Ok(concat_batches(&schema, &parts)?)
    }
}

fn clamp(range: Range<usize>, len: usize) -> Range<usize> {
    let end = range.end.min(len);
    range.start.min(end)..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, Int64Array};
    use arrow::datatypes::{DataType, Field};
    use parquet::arrow::arrow_reader::ArrowReaderOptions;
    use veta_testkit::{TempDir, fixtures};

    fn ids(batch: &RecordBatch) -> Vec<i64> {
        let column = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        column.values().to_vec()
    }

    fn memory(lengths: &[usize]) -> MemorySource {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let mut next = 0;
        let batches = lengths
            .iter()
            .map(|&len| {
                let values = Int64Array::from_iter_values(next..next + len as i64);
                next += len as i64;
                RecordBatch::try_new(schema.clone(), vec![Arc::new(values)]).unwrap()
            })
            .collect();
        MemorySource::new(schema, batches).unwrap()
    }

    #[test]
    fn memory_reads_across_batches() {
        let source = memory(&[3, 0, 4, 2]);
        assert_eq!(source.num_rows(), 9);
        assert_eq!(ids(&source.read(2..6).unwrap()), [2, 3, 4, 5]);
        assert_eq!(ids(&source.read(7..100).unwrap()), [7, 8]);
        assert_eq!(source.read(50..60).unwrap().num_rows(), 0);
    }

    #[test]
    fn empty_source_reads_nothing() {
        let source = MemorySource::empty();
        assert_eq!(source.num_rows(), 0);
        assert_eq!(source.read(0..10).unwrap().num_rows(), 0);
    }

    fn paged(path: &std::path::Path, budget: usize) -> PagedSource {
        let file = File::open(path).unwrap();
        let options = ArrowReaderOptions::new()
            .with_page_index_policy(parquet::file::metadata::PageIndexPolicy::Optional);
        let metadata = ArrowReaderMetadata::load(&file, options).unwrap();
        PagedSource::new(file, metadata, budget).with_path(path)
    }

    #[test]
    fn paged_reads_across_chunks_and_row_groups() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        // Row groups of 100k rows: two chunks per group, the second partial.
        fixtures::large(&path, 250_000, 100_000);
        let source = paged(&path, usize::MAX);
        assert_eq!(source.num_rows(), 250_000);

        let batch = source.read(CHUNK_ROWS - 2..CHUNK_ROWS + 2).unwrap();
        let expected: Vec<i64> = (CHUNK_ROWS as i64 - 2..CHUNK_ROWS as i64 + 2).collect();
        assert_eq!(ids(&batch), expected);

        let batch = source.read(99_998..100_003).unwrap();
        assert_eq!(ids(&batch), [99_998, 99_999, 100_000, 100_001, 100_002]);

        let batch = source.read(249_999..300_000).unwrap();
        assert_eq!(ids(&batch), [249_999]);
    }

    #[test]
    fn paged_reads_the_same_in_any_order() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        // Row groups of 300k rows: several chunks per group.
        fixtures::large(&path, 700_000, 300_000);
        let whole: Vec<i64> = (0..700_000).collect();
        // In order (one reader continued per row group), in reverse, and
        // jumping back and forth, with no cache to fall back on.
        let source = paged(&path, 0);
        let step = 50_000;
        let mut forward = Vec::new();
        for start in (0..700_000).step_by(step) {
            forward.extend(ids(&source.read(start..start + step).unwrap()));
        }
        assert_eq!(forward, whole);
        let source = paged(&path, 0);
        let mut backward = Vec::new();
        for start in (0..700_000).step_by(step).rev() {
            let mut part = ids(&source.read(start..start + step).unwrap());
            part.extend(backward);
            backward = part;
        }
        assert_eq!(backward, whole);
        let source = paged(&path, 0);
        for start in [400_000, 10, 650_000, 70_000, 400_000] {
            let expected: Vec<i64> = (start as i64..start as i64 + 1000).collect();
            assert_eq!(ids(&source.read(start..start + 1000).unwrap()), expected);
        }
        // Without the path (one handle, one reader): the same rows.
        let file = File::open(&path).unwrap();
        let metadata = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new()).unwrap();
        let single = PagedSource::new(file, metadata, 0);
        let mut rows = Vec::new();
        for start in (0..700_000).step_by(step) {
            rows.extend(ids(&single.read(start..start + step).unwrap()));
        }
        assert_eq!(rows, whole);
        // Only some columns: the same values.
        let names = ["label".to_string(), "id".to_string()];
        let projected = source.read_columns(299_000..301_000, &names).unwrap();
        let full = source.read(299_000..301_000).unwrap();
        assert_eq!(projected.num_columns(), 2);
        assert_eq!(
            projected.schema().field(0).name(),
            "id",
            "in the schema's order"
        );
        assert_eq!(projected, project(&full, &names).unwrap());
    }

    #[test]
    fn paged_stays_within_budget_when_scanning() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        fixtures::large(&path, 400_000, 200_000);

        // Measure one chunk, then allow roughly two.
        let probe = paged(&path, usize::MAX);
        probe.read(0..1).unwrap();
        let chunk_bytes = probe.cached_bytes();
        let budget = chunk_bytes * 2 + chunk_bytes / 2;

        let source = paged(&path, budget);
        let mut start = 0;
        while start < source.num_rows() {
            let batch = source.read(start..start + 1000).unwrap();
            assert_eq!(ids(&batch)[0], start as i64);
            assert!(source.cached_bytes() <= budget, "cache exceeded budget");
            start += 7919; // stride that crosses chunk boundaries unevenly
        }
    }

    #[test]
    fn paged_keeps_one_chunk_even_with_zero_budget() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        fixtures::large(&path, 10_000, 5_000);
        let source = paged(&path, 0);
        assert_eq!(ids(&source.read(4_999..5_001).unwrap()), [4_999, 5_000]);
        assert!(source.cached_bytes() > 0);
        assert_eq!(source.read(0..0).unwrap().num_rows(), 0);
        assert_eq!(source.read(0..3).unwrap().column(0).len(), 3);
    }
}
