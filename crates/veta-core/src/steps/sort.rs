//! Sorting rows by one or more columns.
//!
//! Small (in-memory) inputs are sorted in memory. Paged inputs are sorted
//! externally: sorted runs are written to temporary Parquet files and merged
//! into a checkpoint file that later steps read on demand.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::sync::Arc;

use arrow::array::{ArrayRef, UInt32Array};
use arrow::compute::kernels::interleave::interleave_record_batch;
use arrow::compute::{SortOptions, concat_batches, take_record_batch};
use arrow::datatypes::{Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use arrow::row::{OwnedRow, RowConverter, Rows, SortField};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};

use super::checkpoint::{Checkpoint, TempFile};
use super::invalid;
use crate::error::{Error, Result};

/// Sorts rows by `keys`, the first key first. The sort is stable and nulls
/// come last.
#[derive(Debug, Clone, PartialEq)]
pub struct Sort {
    pub keys: Vec<SortKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub column: String,
    pub descending: bool,
}

/// Approximate decoded size of one sorted run.
const RUN_BYTES: usize = 256 * 1024 * 1024;
/// Rows per batch written to the checkpoint file.
const OUTPUT_ROWS: usize = 16 * 1024;

impl Sort {
    pub fn describe(&self) -> String {
        let keys: Vec<String> = self
            .keys
            .iter()
            .map(|k| {
                let order = if k.descending {
                    "descending"
                } else {
                    "ascending"
                };
                format!("{} {order}", k.column)
            })
            .collect();
        format!("Sorted by {}", keys.join(", then "))
    }

    /// Checks the sort against the input schema.
    pub(super) fn validate(&self, schema: &Schema) -> Result<()> {
        self.converter(schema).map(|_| ())
    }

    fn columns(&self, schema: &Schema) -> Result<Vec<usize>> {
        if self.keys.is_empty() {
            return Err(invalid("a sort needs at least one column".into()));
        }
        self.keys
            .iter()
            .map(|k| {
                schema
                    .index_of(&k.column)
                    .map_err(|_| invalid(format!("no column named {:?}", k.column)))
            })
            .collect()
    }

    fn converter(&self, schema: &Schema) -> Result<RowConverter> {
        let fields = self
            .columns(schema)?
            .into_iter()
            .zip(&self.keys)
            .map(|(i, k)| {
                let options = SortOptions {
                    descending: k.descending,
                    nulls_first: false,
                };
                SortField::new_with_options(schema.field(i).data_type().clone(), options)
            })
            .collect::<Vec<_>>();
        if !RowConverter::supports_fields(&fields) {
            return Err(invalid("one of the sort columns can't be sorted".into()));
        }
        Ok(RowConverter::new(fields)?)
    }
}

/// Sort state shared by the in-memory and external sorts.
struct Sorter {
    columns: Vec<usize>,
    converter: RowConverter,
}

impl Sorter {
    fn new(sort: &Sort, schema: &Schema) -> Result<Self> {
        Ok(Self {
            columns: sort.columns(schema)?,
            converter: sort.converter(schema)?,
        })
    }

    fn rows(&self, batch: &RecordBatch) -> Result<Rows> {
        let keys: Vec<ArrayRef> = self
            .columns
            .iter()
            .map(|&i| batch.column(i).clone())
            .collect();
        Ok(self.converter.convert_columns(&keys)?)
    }

    /// Sorts one batch (stable).
    fn sort(&self, batch: &RecordBatch) -> Result<RecordBatch> {
        let rows = self.rows(batch)?;
        let mut indices: Vec<u32> = (0..batch.num_rows() as u32).collect();
        indices.sort_by(|&a, &b| rows.row(a as usize).cmp(&rows.row(b as usize)));
        Ok(take_record_batch(batch, &UInt32Array::from(indices))?)
    }
}

/// Reads input rows in `range` of the step being sorted.
pub(super) type ReadInput<'a> = dyn Fn(std::ops::Range<usize>) -> Result<RecordBatch> + 'a;

/// Sorts all of the input in memory.
pub(super) fn sort_in_memory(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<RecordBatch> {
    let sorter = Sorter::new(sort, schema)?;
    let batch = read_all(schema, 0..num_rows, read, batch_rows, &mut |done| {
        progress(0.9 * done as f32 / num_rows.max(1) as f32)
    })?;
    let sorted = sorter.sort(&batch)?;
    if !progress(1.0) {
        return Err(Error::Cancelled);
    }
    Ok(sorted)
}

/// Sorts the input into a temporary Parquet file, in runs of about
/// `run_rows` rows (estimated from the data when `None`).
pub(super) fn sort_external(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
    run_rows: Option<usize>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Checkpoint> {
    let sorter = Sorter::new(sort, schema)?;
    let run_rows = match run_rows {
        Some(rows) => rows.max(1),
        None => {
            let sample = read(0..batch_rows.min(num_rows))?;
            let per_row = sample.get_array_memory_size() / sample.num_rows().max(1);
            (RUN_BYTES / per_row.max(1)).max(batch_rows)
        }
    };
    let runs_count = num_rows.div_ceil(run_rows).max(1);
    // Reading and sorting runs is half the work when there is a merge.
    let read_share = if runs_count > 1 { 0.5 } else { 0.95 };

    let mut runs = Vec::with_capacity(runs_count);
    let mut start = 0;
    loop {
        let end = (start + run_rows).min(num_rows);
        let batch = read_all(schema, start..end, read, batch_rows, &mut |done| {
            progress(read_share * (start + done) as f32 / num_rows.max(1) as f32)
        })?;
        let file = TempFile::new()?;
        write_parquet(&file, schema, std::iter::once(Ok(sorter.sort(&batch)?)))?;
        runs.push(file);
        start = end;
        if start >= num_rows {
            break;
        }
    }

    let output = if runs.len() == 1 {
        runs.pop().expect("one run")
    } else {
        let output = TempFile::new()?;
        let merged = Merge::new(&sorter, &runs, num_rows)?;
        let mut written = 0;
        write_parquet(
            &output,
            schema,
            merged.map(|batch| {
                let batch = batch?;
                written += batch.num_rows();
                let fraction = written as f32 / num_rows.max(1) as f32;
                if progress(read_share + (0.95 - read_share) * fraction) {
                    Ok(batch)
                } else {
                    Err(Error::Cancelled)
                }
            }),
        )?;
        output
    };
    let checkpoint = Checkpoint::open(output, schema.clone())?;
    if !progress(1.0) {
        return Err(Error::Cancelled);
    }
    Ok(checkpoint)
}

/// Reads `range` in batches of `batch_rows` and concatenates them.
/// `progress` gets the rows read so far and returns `false` to cancel.
fn read_all(
    schema: &SchemaRef,
    range: std::ops::Range<usize>,
    read: &ReadInput<'_>,
    batch_rows: usize,
    progress: &mut dyn FnMut(usize) -> bool,
) -> Result<RecordBatch> {
    let mut batches = Vec::new();
    let mut start = range.start;
    while start < range.end {
        let end = (start + batch_rows).min(range.end);
        batches.push(read(start..end)?);
        start = end;
        if !progress(start - range.start) {
            return Err(Error::Cancelled);
        }
    }
    Ok(concat_batches(schema, &batches)?)
}

fn write_parquet(
    file: &TempFile,
    schema: &SchemaRef,
    batches: impl Iterator<Item = Result<RecordBatch>>,
) -> Result<()> {
    let mut writer = ArrowWriter::try_new(File::create(file.path())?, schema.clone(), None)?;
    for batch in batches {
        let batch = batch?;
        // Concatenated batches may carry different schema metadata.
        let batch = RecordBatch::try_new(schema.clone(), batch.columns().to_vec())?;
        writer.write(&batch)?;
    }
    writer.close()?;
    Ok(())
}

/// A k-way merge of sorted runs, yielding sorted batches.
struct Merge<'a> {
    sorter: &'a Sorter,
    runs: Vec<Run>,
    /// Next row of each non-exhausted run, smallest first; ties go to the
    /// earlier run, which keeps the sort stable.
    heap: BinaryHeap<Reverse<(OwnedRow, usize)>>,
    remaining: usize,
}

struct Run {
    reader: ParquetRecordBatchReader,
    batch: RecordBatch,
    rows: Rows,
    next: usize,
}

impl<'a> Merge<'a> {
    fn new(sorter: &'a Sorter, files: &[TempFile], total: usize) -> Result<Self> {
        // Keep the batches read from all runs at once within a run's size.
        let row_bytes = 64;
        let batch_rows = (RUN_BYTES / files.len() / row_bytes).clamp(256, 8192);
        let mut merge = Self {
            sorter,
            runs: Vec::with_capacity(files.len()),
            heap: BinaryHeap::with_capacity(files.len()),
            remaining: total,
        };
        for (i, file) in files.iter().enumerate() {
            let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(file.path())?)?
                .with_batch_size(batch_rows)
                .build()?;
            let mut run = Run {
                reader,
                batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
                rows: sorter.converter.empty_rows(0, 0),
                next: 0,
            };
            if advance(sorter, &mut run)? {
                merge.heap.push(Reverse((run.rows.row(0).owned(), i)));
            }
            merge.runs.push(run);
        }
        Ok(merge)
    }

    fn next_batch(&mut self) -> Result<RecordBatch> {
        // Batches referenced by this output batch, and (batch, row) picks.
        let mut sources: Vec<RecordBatch> = Vec::new();
        let mut source_of_run: Vec<Option<usize>> = vec![None; self.runs.len()];
        let mut picks: Vec<(usize, usize)> = Vec::with_capacity(OUTPUT_ROWS);
        while picks.len() < OUTPUT_ROWS {
            let Some(Reverse((_, i))) = self.heap.pop() else {
                break;
            };
            let source = match source_of_run[i] {
                Some(s) => s,
                None => {
                    sources.push(self.runs[i].batch.clone());
                    source_of_run[i] = Some(sources.len() - 1);
                    sources.len() - 1
                }
            };
            let run = &mut self.runs[i];
            picks.push((source, run.next));
            run.next += 1;
            let more = if run.next < run.batch.num_rows() {
                true
            } else if advance(self.sorter, run)? {
                // A new batch: later picks from this run refer to it.
                source_of_run[i] = None;
                true
            } else {
                false
            };
            if more {
                self.heap.push(Reverse((run.rows.row(run.next).owned(), i)));
            }
        }
        self.remaining -= picks.len();
        let refs: Vec<&RecordBatch> = sources.iter().collect();
        Ok(interleave_record_batch(&refs, &picks)?)
    }
}

impl Iterator for Merge<'_> {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.heap.is_empty() {
            return None;
        }
        Some(self.next_batch())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let batches = self.remaining.div_ceil(OUTPUT_ROWS);
        (batches, Some(batches))
    }
}

/// Loads the run's next batch. Returns `false` when it is exhausted.
fn advance(sorter: &Sorter, run: &mut Run) -> Result<bool> {
    for batch in run.reader.by_ref() {
        let batch = batch?;
        if batch.num_rows() == 0 {
            continue;
        }
        run.rows = sorter.rows(&batch)?;
        run.batch = batch;
        run.next = 0;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field};

    use crate::source::DataSource;

    fn input(n: usize) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("group", DataType::Utf8, true),
            Field::new("score", DataType::Int64, true),
        ]));
        let groups = ["b", "a", "c"];
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from_iter_values(0..n as i64)),
                Arc::new(StringArray::from_iter(
                    (0..n).map(|i| (i % 10 != 9).then(|| groups[(i * 7) % 3])),
                )),
                Arc::new(Int64Array::from_iter(
                    (0..n).map(|i| (i % 13 != 0).then_some(((i * 37) % 11) as i64)),
                )),
            ],
        )
        .unwrap()
    }

    fn key(column: &str, descending: bool) -> SortKey {
        SortKey {
            column: column.into(),
            descending,
        }
    }

    fn ints(batch: &RecordBatch, column: &str) -> Vec<Option<i64>> {
        let array = batch.column_by_name(column).unwrap();
        let array = array.as_any().downcast_ref::<Int64Array>().unwrap();
        array.iter().collect()
    }

    fn strings(batch: &RecordBatch, column: &str) -> Vec<Option<String>> {
        let array = batch.column_by_name(column).unwrap();
        let array = array.as_any().downcast_ref::<StringArray>().unwrap();
        array.iter().map(|s| s.map(str::to_owned)).collect()
    }

    /// The expected order, computed the obvious way.
    fn expected(batch: &RecordBatch, sort: &Sort) -> Vec<Option<i64>> {
        let groups = strings(batch, "group");
        let scores = ints(batch, "score");
        let mut order: Vec<usize> = (0..batch.num_rows()).collect();
        order.sort_by(|&a, &b| {
            for k in &sort.keys {
                let ord = match k.column.as_str() {
                    "group" => cmp_nulls_last(&groups[a], &groups[b], k.descending),
                    _ => cmp_nulls_last(&scores[a], &scores[b], k.descending),
                };
                if ord.is_ne() {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
        order.into_iter().map(|i| Some(i as i64)).collect()
    }

    fn cmp_nulls_last<T: Ord>(a: &Option<T>, b: &Option<T>, desc: bool) -> std::cmp::Ordering {
        match (a, b) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, _) => std::cmp::Ordering::Greater,
            (_, None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) if desc => b.cmp(a),
            (Some(a), Some(b)) => a.cmp(b),
        }
    }

    fn sorts() -> Vec<Sort> {
        vec![
            Sort {
                keys: vec![key("score", false)],
            },
            Sort {
                keys: vec![key("group", false), key("score", true)],
            },
            Sort {
                keys: vec![key("group", true)],
            },
        ]
    }

    #[test]
    fn in_memory_sort_is_stable_with_nulls_last() {
        let batch = input(200);
        let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
        for sort in sorts() {
            let sorted =
                sort_in_memory(&sort, &batch.schema(), 200, &read, 64, &mut |_| true).unwrap();
            assert_eq!(ints(&sorted, "id"), expected(&batch, &sort), "{sort:?}");
        }
    }

    #[test]
    fn external_sort_merges_runs_into_a_temp_file() {
        let batch = input(1000);
        let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
        for (sort, run_rows) in sorts().into_iter().zip([1000, 77, 10]) {
            let mut seen = Vec::new();
            let checkpoint = sort_external(
                &sort,
                &batch.schema(),
                1000,
                &read,
                32,
                Some(run_rows),
                &mut |f| {
                    seen.push(f);
                    true
                },
            )
            .unwrap();
            assert_eq!(checkpoint.num_rows(), 1000);
            assert_eq!(checkpoint.schema(), batch.schema());
            let sorted = checkpoint.read(0..1000).unwrap();
            assert_eq!(ints(&sorted, "id"), expected(&batch, &sort), "{sort:?}");
            assert!(seen.windows(2).all(|w| w[0] <= w[1]));
            assert_eq!(seen.last(), Some(&1.0));

            let path = checkpoint.path().to_path_buf();
            assert!(path.exists());
            drop(checkpoint);
            assert!(!path.exists(), "the checkpoint is deleted with its source");
        }
    }

    #[test]
    fn external_sort_can_be_cancelled() {
        let batch = input(100);
        let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
        let sort = &sorts()[0];
        let mut calls = 0;
        let result = sort_external(sort, &batch.schema(), 100, &read, 10, Some(30), &mut |_| {
            calls += 1;
            calls < 5
        });
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn validation_and_description() {
        let schema = input(1).schema();
        let sort = &sorts()[1];
        assert!(sort.validate(&schema).is_ok());
        assert_eq!(
            sort.describe(),
            "Sorted by group ascending, then score descending"
        );
        assert!(Sort { keys: vec![] }.validate(&schema).is_err());
        assert!(
            Sort {
                keys: vec![key("nope", false)]
            }
            .validate(&schema)
            .is_err()
        );
    }
}
