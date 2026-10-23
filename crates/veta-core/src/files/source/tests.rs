//! Tests for `files::source`.

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
