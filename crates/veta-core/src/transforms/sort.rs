//! Sorting rows by one or more columns.
//!
//! Small (in-memory) inputs are sorted in memory. Paged inputs are sorted
//! externally: sorted runs are written to temporary Parquet files and merged,
//! into a checkpoint file that later steps read on demand, or straight into
//! the saved file.

use std::fs::File;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, sync_channel};

use arrow::array::{Array, ArrayRef, UInt32Array};
use arrow::compute::{SortOptions, concat_batches, interleave, take};
use arrow::datatypes::{Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use arrow::row::{RowConverter, Rows, SortField};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::error::{Error, Result};
use crate::steps::checkpoint::{Checkpoint, TempFile, write_parquet};
use crate::steps::invalid;

/// Sorts rows by `keys`, the first key first. The sort is stable.
#[derive(Debug, Clone, PartialEq)]
pub struct Sort {
    pub keys: Vec<SortKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub column: String,
    pub descending: bool,
    /// Nulls before other values (otherwise after).
    pub nulls_first: bool,
}

/// Approximate decoded size of one sorted run.
const RUN_BYTES: usize = 256 * 1024 * 1024;
/// Rows per merged batch.
const OUTPUT_ROWS: usize = 64 * 1024;
/// Rows per slice when converting sort keys on several cores.
const SLICE_ROWS: usize = 64 * 1024;

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
                let nulls = if k.nulls_first { ", nulls first" } else { "" };
                format!("{} {order}{nulls}", k.column)
            })
            .collect();
        format!("Sorted by {}", keys.join(", then "))
    }

    /// Checks the sort against the input schema.
    pub(crate) fn validate(&self, schema: &Schema) -> Result<()> {
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
                    nulls_first: k.nulls_first,
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

    /// The order of the rows of `batches`, numbered across them, that
    /// sorts them (stable). Keys are converted on all cores.
    fn order(&self, batches: &[RecordBatch]) -> Result<Vec<u32>> {
        let parts = crate::parallel::map(batches.len(), |i| self.rows(&batches[i]))?;
        Ok(sorted_order(&parts))
    }

    /// Sorts one batch (stable). Keys are converted in slices and columns
    /// reordered at the same time, on all cores.
    fn sort(&self, batch: &RecordBatch) -> Result<RecordBatch> {
        let n = batch.num_rows();
        let slices: Vec<RecordBatch> = (0..n)
            .step_by(SLICE_ROWS)
            .map(|start| batch.slice(start, SLICE_ROWS.min(n - start)))
            .collect();
        let order = UInt32Array::from(self.order(&slices)?);
        let columns = crate::parallel::map(batch.num_columns(), |c| {
            Ok(take(batch.column(c), &order, None)?)
        })?;
        let options = RecordBatchOptions::new().with_row_count(Some(n));
        Ok(RecordBatch::try_new_with_options(
            batch.schema(),
            columns,
            &options,
        )?)
    }
}

/// The order of the rows in `parts` (row-format sort keys of consecutive
/// slices) that sorts them, stable.
///
/// Ties are broken by the row's position, so an unstable sort (faster) is
/// stable. Keys of at most 16 bytes each (e.g. one number or date column)
/// are sorted as packed integers: row-format keys compare as bytes, and
/// equal-length byte strings compare like the big-endian numbers they spell.
fn sorted_order(parts: &[Rows]) -> Vec<u32> {
    let rows = || parts.iter().flat_map(Rows::iter);
    let width = rows().next().map(|r| r.data().len());
    let fixed = width.filter(|&w| w <= 16 && rows().all(|r| r.data().len() == w));
    if let Some(width) = fixed {
        let mut keys: Vec<(u128, u32)> = rows()
            .enumerate()
            .map(|(i, row)| {
                let mut bytes = [0u8; 16];
                bytes[..width].copy_from_slice(row.data());
                (u128::from_be_bytes(bytes), i as u32)
            })
            .collect();
        keys.sort_unstable();
        return keys.into_iter().map(|(_, i)| i).collect();
    }
    let mut keys: Vec<(&[u8], u32)> = rows()
        .enumerate()
        .map(|(i, row)| (row.data(), i as u32))
        .collect();
    keys.sort_unstable();
    keys.into_iter().map(|(_, i)| i).collect()
}

/// Reads input rows in `range` of the step being sorted.
pub(crate) type ReadInput<'a> = dyn Fn(std::ops::Range<usize>) -> Result<RecordBatch> + 'a;

/// Sorts all of the input in memory.
pub(crate) fn sort_in_memory(
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
pub(crate) fn sort_external(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
    run_rows: Option<usize>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Checkpoint> {
    let mut runs = sort_runs(sort, schema, num_rows, read, batch_rows, run_rows, progress)?;
    // Reading and sorting runs is half the work when there is a merge.
    let read_share = if runs.files.len() > 1 { 0.5 } else { 0.95 };
    let output = if runs.files.len() == 1 {
        runs.files.pop().expect("one run")
    } else {
        let output = TempFile::new()?;
        let merged = Merge::new(runs)?;
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

/// Sorts the input like [`sort_external`], but returns the merge of the
/// runs, to be read once in order, instead of writing it to a file.
pub(crate) fn sort_stream(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
) -> Result<Merge> {
    Merge::new(sort_runs(
        sort,
        schema,
        num_rows,
        read,
        batch_rows,
        None,
        &mut |_| true,
    )?)
}

/// Sorted runs of the input, in temporary files.
struct Runs {
    sorter: Arc<Sorter>,
    schema: SchemaRef,
    files: Vec<TempFile>,
    num_rows: usize,
}

/// Reads and sorts the input in runs. Each run is written to its file on
/// another thread while the next one is read.
fn sort_runs(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
    run_rows: Option<usize>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Runs> {
    let sorter = Arc::new(Sorter::new(sort, schema)?);
    let run_rows = match run_rows {
        Some(rows) => rows.max(1),
        None => {
            let sample = read(0..batch_rows.min(num_rows))?;
            let per_row = sample.get_array_memory_size() / sample.num_rows().max(1);
            (RUN_BYTES / per_row.max(1)).max(batch_rows)
        }
    };
    let runs_count = num_rows.div_ceil(run_rows).max(1);
    let read_share = if runs_count > 1 { 0.5 } else { 0.95 };

    let mut files = Vec::with_capacity(runs_count);
    let mut writing: Option<std::thread::JoinHandle<Result<TempFile>>> = None;
    let finish = |writing: Option<std::thread::JoinHandle<Result<TempFile>>>| match writing {
        Some(w) => w
            .join()
            .unwrap_or_else(|_| Err(invalid("sorting failed".into())))
            .map(Some),
        None => Ok(None),
    };
    let mut start = 0;
    loop {
        let end = (start + run_rows).min(num_rows);
        let batches = read_batches(start..end, read, batch_rows, &mut |done| {
            progress(read_share * (start + done) as f32 / num_rows.max(1) as f32)
        });
        let order = batches.and_then(|b| Ok((sorter.order(&b)?, b)));
        // The previous run is written by now, or the error is reported.
        let written = finish(writing.take());
        let (order, batches) = order?;
        files.extend(written?);
        let file = TempFile::new()?;
        let schema = schema.clone();
        // Sorted rows are gathered a batch at a time as they are written,
        // never all at once.
        writing = Some(std::thread::spawn(move || {
            let starts: Vec<usize> = batches
                .iter()
                .scan(0, |next, b| {
                    let start = *next;
                    *next += b.num_rows();
                    Some(start)
                })
                .collect();
            let sorted = order.chunks(OUTPUT_ROWS).map(|chunk| {
                let picks: Vec<(usize, usize)> = chunk
                    .iter()
                    .map(|&row| {
                        let row = row as usize;
                        let batch = starts.partition_point(|&s| s <= row) - 1;
                        (batch, row - starts[batch])
                    })
                    .collect();
                gather(&schema, &batches, &picks)
            });
            write_parquet(&file, &schema, sorted)?;
            Ok(file)
        }));
        start = end;
        if start >= num_rows {
            break;
        }
    }
    files.extend(finish(writing)?);
    Ok(Runs {
        sorter,
        schema: schema.clone(),
        files,
        num_rows,
    })
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
    let batches = read_batches(range, read, batch_rows, progress)?;
    Ok(concat_batches(schema, &batches)?)
}

/// Rows `picks` (batch, row) of `batches`, in that order, gathering the
/// columns at the same time.
fn gather(
    schema: &SchemaRef,
    batches: &[RecordBatch],
    picks: &[(usize, usize)],
) -> Result<RecordBatch> {
    let columns = crate::parallel::map(schema.fields().len(), |c| {
        let arrays: Vec<&dyn Array> = batches.iter().map(|b| b.column(c).as_ref()).collect();
        Ok(interleave(&arrays, picks)?)
    })?;
    let options = RecordBatchOptions::new().with_row_count(Some(picks.len()));
    Ok(RecordBatch::try_new_with_options(
        schema.clone(),
        columns,
        &options,
    )?)
}

/// Reads `range` in batches of `batch_rows`. `progress` gets the rows read
/// so far and returns `false` to cancel.
fn read_batches(
    range: std::ops::Range<usize>,
    read: &ReadInput<'_>,
    batch_rows: usize,
    progress: &mut dyn FnMut(usize) -> bool,
) -> Result<Vec<RecordBatch>> {
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
    Ok(batches)
}

/// A k-way merge of sorted runs, yielding sorted batches.
pub(crate) struct Merge {
    schema: SchemaRef,
    runs: Vec<Run>,
    /// Runs with rows left, as a binary heap on each run's next row,
    /// smallest first; ties go to the earlier run, which keeps the sort
    /// stable. Rows are compared where they are, without copies.
    heap: Vec<usize>,
    remaining: usize,
    /// The run files, deleted with the merge (after `runs`, whose readers
    /// stop when it is dropped).
    _files: Vec<TempFile>,
}

struct Run {
    /// The run's batches with their sort keys, decoded on another thread
    /// one batch ahead.
    batches: Receiver<Result<(RecordBatch, Rows)>>,
    batch: RecordBatch,
    rows: Rows,
    next: usize,
}

impl Merge {
    fn new(runs: Runs) -> Result<Self> {
        let Runs {
            sorter,
            schema,
            files,
            num_rows,
        } = runs;
        // Keep the batches read from all runs at once within a run's size.
        let row_bytes = 64;
        let batch_rows = (RUN_BYTES / files.len().max(1) / row_bytes).clamp(256, 8192);
        let mut merge = Self {
            schema,
            runs: Vec::with_capacity(files.len()),
            heap: Vec::with_capacity(files.len()),
            remaining: num_rows,
            _files: Vec::new(),
        };
        for (i, file) in files.iter().enumerate() {
            let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(file.path())?)?
                .with_batch_size(batch_rows)
                .build()?;
            let (send, batches) = sync_channel(1);
            let keys = sorter.clone();
            std::thread::spawn(move || {
                for batch in reader {
                    let item = batch.map_err(Error::from).and_then(|batch| {
                        let rows = keys.rows(&batch)?;
                        Ok((batch, rows))
                    });
                    if item.as_ref().is_ok_and(|(b, _)| b.num_rows() == 0) {
                        continue;
                    }
                    if send.send(item).is_err() {
                        break;
                    }
                }
            });
            let mut run = Run {
                batches,
                batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
                rows: sorter.converter.empty_rows(0, 0),
                next: 0,
            };
            let has_rows = advance(&mut run)?;
            merge.runs.push(run);
            if has_rows {
                merge.heap.push(i);
                merge.sift_up(merge.heap.len() - 1);
            }
        }
        merge._files = files;
        Ok(merge)
    }

    /// Whether run `a`'s next row comes before run `b`'s.
    fn before(&self, a: usize, b: usize) -> bool {
        let (ra, rb) = (&self.runs[a], &self.runs[b]);
        match ra.rows.row(ra.next).cmp(&rb.rows.row(rb.next)) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
            std::cmp::Ordering::Equal => a < b,
        }
    }

    fn sift_up(&mut self, mut i: usize) {
        while i > 0 {
            let parent = (i - 1) / 2;
            if !self.before(self.heap[i], self.heap[parent]) {
                break;
            }
            self.heap.swap(i, parent);
            i = parent;
        }
    }

    fn sift_down(&mut self, mut i: usize) {
        loop {
            let (left, right) = (2 * i + 1, 2 * i + 2);
            let mut smallest = i;
            if left < self.heap.len() && self.before(self.heap[left], self.heap[smallest]) {
                smallest = left;
            }
            if right < self.heap.len() && self.before(self.heap[right], self.heap[smallest]) {
                smallest = right;
            }
            if smallest == i {
                break;
            }
            self.heap.swap(i, smallest);
            i = smallest;
        }
    }

    fn next_batch(&mut self) -> Result<RecordBatch> {
        // Batches referenced by this output batch, and (batch, row) picks.
        let mut sources: Vec<RecordBatch> = Vec::new();
        let mut source_of_run: Vec<Option<usize>> = vec![None; self.runs.len()];
        let mut picks: Vec<(usize, usize)> = Vec::with_capacity(OUTPUT_ROWS);
        while picks.len() < OUTPUT_ROWS {
            let Some(&i) = self.heap.first() else {
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
            } else if advance(run)? {
                // A new batch: later picks from this run refer to it.
                source_of_run[i] = None;
                true
            } else {
                false
            };
            if !more {
                self.heap.swap_remove(0);
            }
            self.sift_down(0);
        }
        self.remaining -= picks.len();
        gather(&self.schema, &sources, &picks)
    }
}

impl Iterator for Merge {
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
fn advance(run: &mut Run) -> Result<bool> {
    match run.batches.recv() {
        Ok(next) => {
            (run.batch, run.rows) = next?;
            run.next = 0;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
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
            nulls_first: false,
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
    fn nulls_first_per_key() {
        let batch = input(100);
        let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
        let sort = Sort {
            keys: vec![SortKey {
                column: "score".into(),
                descending: true,
                nulls_first: true,
            }],
        };
        assert_eq!(sort.describe(), "Sorted by score descending, nulls first");
        let sorted = sort_in_memory(&sort, &batch.schema(), 100, &read, 64, &mut |_| true).unwrap();
        let scores = ints(&sorted, "score");
        let nulls = scores.iter().filter(|s| s.is_none()).count();
        assert!(nulls > 0);
        assert!(scores[..nulls].iter().all(Option::is_none));
        assert_eq!(scores[nulls], Some(10));
        // The external sort agrees.
        let checkpoint = sort_external(
            &sort,
            &batch.schema(),
            100,
            &read,
            16,
            Some(30),
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(ints(&checkpoint.read(0..100).unwrap(), "score"), scores);
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
