//! Data sources: where a document's original rows come from.
//!
//! [`MemorySource`] holds everything in memory. [`PagedSource`] reads a
//! Parquet file in chunks on demand and keeps recently used chunks in a cache
//! bounded by a memory budget.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::File;
use std::ops::Range;
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
            return Err(Error::Arrow(arrow::error::ArrowError::SchemaError(
                format!(
                    "batch schema {:?} does not match source schema",
                    bad.schema()
                ),
            )));
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
    chunks: Vec<ChunkSpan>,
    num_rows: usize,
    budget: usize,
    cache: Mutex<ChunkCache>,
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
        let mut chunks = Vec::new();
        let mut start = 0;
        for (row_group, rg) in metadata.metadata().row_groups().iter().enumerate() {
            let rows = usize::try_from(rg.num_rows()).unwrap_or(0);
            let mut offset = 0;
            while offset < rows {
                let len = CHUNK_ROWS.min(rows - offset);
                chunks.push(ChunkSpan {
                    row_group,
                    offset_in_group: offset,
                    start: start + offset,
                    len,
                });
                offset += len;
            }
            start += rows;
        }
        Self {
            schema: metadata.schema().clone(),
            file,
            metadata,
            chunks,
            num_rows: start,
            budget,
            cache: Mutex::new(ChunkCache::default()),
        }
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

        let batch = self.load_chunk(self.chunks[index])?;

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

    fn load_chunk(&self, span: ChunkSpan) -> Result<RecordBatch> {
        let group_rows = usize::try_from(
            self.metadata
                .metadata()
                .row_group(span.row_group)
                .num_rows(),
        )
        .unwrap_or(0);
        let mut selectors = Vec::with_capacity(3);
        if span.offset_in_group > 0 {
            selectors.push(RowSelector::skip(span.offset_in_group));
        }
        selectors.push(RowSelector::select(span.len));
        let rest = group_rows - span.offset_in_group - span.len;
        if rest > 0 {
            selectors.push(RowSelector::skip(rest));
        }

        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(
            self.file.try_clone()?,
            self.metadata.clone(),
        )
        .with_row_groups(vec![span.row_group])
        .with_row_selection(RowSelection::from(selectors))
        .with_batch_size(span.len)
        .build()?;
        let batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(concat_batches(&self.schema, &batches)?)
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
        let first = self
            .chunks
            .partition_point(|c| c.start + c.len <= range.start);
        let mut parts = Vec::new();
        for (index, span) in self.chunks.iter().enumerate().skip(first) {
            if span.start >= range.end {
                break;
            }
            let batch = self.chunk(index)?;
            let from = range.start.max(span.start) - span.start;
            let to = range.end.min(span.start + span.len) - span.start;
            parts.push(batch.slice(from, to - from));
        }
        Ok(concat_batches(&self.schema, &parts)?)
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
        PagedSource::new(file, metadata, budget)
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
