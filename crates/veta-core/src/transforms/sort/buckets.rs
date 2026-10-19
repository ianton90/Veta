//! Sorting by key range (a sample sort). Keys sampled from the input pick
//! cut values that split the key range into buckets of about the same size.
//! Rows go to their bucket as they are read; then each bucket is sorted on
//! its own, on all cores, and the buckets come out in order. Nothing is
//! merged, and each sort works on a bucket small enough to stay mostly in
//! the CPU's cache. Buckets stay in memory when the input fits the memory
//! budget; otherwise they are written to temporary files in parts.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel, sync_channel};
use std::sync::{Arc, Condvar, Mutex};

use arrow::array::{Array, UInt32Array};
use arrow::compute::{interleave, take_record_batch};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use arrow::row::Rows;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use super::{OUTPUT_ROWS, ReadInput, Sort, Sorter, sorted_order};
use crate::error::{Error, Result};
use crate::steps::checkpoint::{TempFile, write_part};

/// Decoded size of a bucket to aim for.
const BUCKET_BYTES: usize = 48 * 1024 * 1024;
const MAX_BUCKETS: usize = 4096;
/// Keys are sampled in this many windows of this many rows, spread over
/// the input.
const SAMPLE_WINDOWS: usize = 64;
const SAMPLE_ROWS: usize = 2048;

/// How the input is split into buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Buckets to cut the key range into.
    pub(crate) buckets: usize,
    /// Buckets are written to files whenever they hold this many bytes;
    /// `None` keeps them in memory.
    pub(crate) flush_bytes: Option<usize>,
}

impl Plan {
    /// The plan for an input of about `bytes` decoded bytes: in memory when
    /// it fits half the memory budget.
    fn for_input(bytes: usize) -> Self {
        let threads = crate::parallel::threads();
        let budget = crate::memory::budget();
        Self {
            buckets: (bytes / BUCKET_BYTES).clamp(2 * threads, MAX_BUCKETS),
            flush_bytes: (bytes > budget / 2).then(|| (budget / 4).max(crate::memory::MIN_BUDGET)),
        }
    }
}

/// A bucket's rows, in input order: parts written to files first, then
/// the rows still in memory.
#[derive(Default)]
struct Bucket {
    parts: Vec<TempFile>,
    memory: Vec<RecordBatch>,
    /// Holds a single key value (or only nulls): already in order.
    single_key: bool,
}

/// Sorts the input by key range. See the module documentation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sort_buckets(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    read_keys: &ReadInput<'_>,
    batch_rows: usize,
    plan: Option<Plan>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Sorted> {
    let sorter = Arc::new(Sorter::new(sort, schema)?);
    let plan = match plan {
        Some(plan) => plan,
        None => {
            let sample = read(0..batch_rows.min(num_rows))?;
            let per_row = sample.get_array_memory_size() / sample.num_rows().max(1);
            Plan::for_input(per_row.saturating_mul(num_rows))
        }
    };
    let cuts = cut_keys(&sorter, num_rows, read_keys, plan.buckets.max(1))?;
    let buckets = fill_buckets(
        &sorter, schema, &cuts, num_rows, read, batch_rows, plan, progress,
    )?;
    Ok(Sorted::start(sorter, schema.clone(), buckets))
}

/// The cut values (row-format keys, in order, no two equal) between buckets
/// of about the same number of rows, from keys sampled over the input.
fn cut_keys(
    sorter: &Sorter,
    num_rows: usize,
    read_keys: &ReadInput<'_>,
    buckets: usize,
) -> Result<Vec<Vec<u8>>> {
    let windows = SAMPLE_WINDOWS.min(num_rows.div_ceil(SAMPLE_ROWS)).max(1);
    let mut sample: Vec<Vec<u8>> = Vec::new();
    for w in 0..windows {
        let start = (num_rows as u128 * w as u128 / windows as u128) as usize;
        let end = (start + SAMPLE_ROWS).min(num_rows);
        if start < end {
            let rows = sorter.rows(&read_keys(start..end)?)?;
            sample.extend(rows.iter().map(|r| r.data().to_vec()));
        }
    }
    sample.sort_unstable();
    let mut cuts: Vec<Vec<u8>> = (1..buckets)
        .filter_map(|i| sample.get(i * sample.len() / buckets).cloned())
        .collect();
    cuts.dedup();
    Ok(cuts)
}

/// The bucket of `key`: `2i` for keys between cut `i - 1` and cut `i`,
/// `2i + 1` for keys equal to cut `i` (a bucket of one key value).
fn bucket_of(cuts: &[Vec<u8>], key: &[u8]) -> usize {
    let i = cuts.partition_point(|c| c.as_slice() < key);
    if cuts.get(i).is_some_and(|c| c.as_slice() == key) {
        2 * i + 1
    } else {
        2 * i
    }
}

/// `batch`'s rows by bucket, each bucket's rows in their input order.
fn split(
    sorter: &Sorter,
    cuts: &[Vec<u8>],
    count: usize,
    batch: &RecordBatch,
) -> Result<Vec<(usize, RecordBatch)>> {
    let rows = sorter.rows(batch)?;
    let ids: Vec<usize> = rows.iter().map(|r| bucket_of(cuts, r.data())).collect();
    let mut counts = vec![0usize; count];
    for &b in &ids {
        counts[b] += 1;
    }
    let used: Vec<usize> = (0..count).filter(|&b| counts[b] > 0).collect();
    if let [only] = used[..] {
        return Ok(vec![(only, batch.clone())]);
    }
    // A counting sort by bucket keeps each bucket's rows in order.
    let mut starts = vec![0usize; count];
    let mut next = 0;
    for b in 0..count {
        starts[b] = next;
        next += counts[b];
    }
    let mut placed = starts.clone();
    let mut order = vec![0u32; ids.len()];
    for (row, &b) in ids.iter().enumerate() {
        order[placed[b]] = row as u32;
        placed[b] += 1;
    }
    let grouped = take_record_batch(batch, &UInt32Array::from(order))?;
    Ok(used
        .into_iter()
        .map(|b| (b, grouped.slice(starts[b], counts[b])))
        .collect())
}

/// Reads the input in order and sends each row to its bucket. Batches are
/// split on all cores while the next ones are read; with a `flush_bytes`
/// limit, buckets are written to temporary files whenever they hold that
/// much. `progress` gets the fraction read.
#[allow(clippy::too_many_arguments)]
fn fill_buckets(
    sorter: &Sorter,
    schema: &SchemaRef,
    cuts: &[Vec<u8>],
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
    plan: Plan,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<Bucket>> {
    let count = 2 * cuts.len() + 1;
    let mut buckets: Vec<Bucket> = (0..count)
        .map(|b| Bucket {
            single_key: b % 2 == 1,
            ..Bucket::default()
        })
        .collect();
    let threads = crate::parallel::threads();
    let (jobs, queue) = sync_channel::<(usize, RecordBatch)>(2 * threads);
    let queue = Mutex::new(queue);
    let (done, results) = channel();
    // Dropped to end the workers and the results.
    let mut jobs = Some(jobs);
    let mut done = Some(done);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (queue, done) = (&queue, done.clone().expect("not dropped yet"));
            scope.spawn(move || {
                loop {
                    let job = queue
                        .lock()
                        .map_err(|_| ())
                        .and_then(|q| q.recv().map_err(|_| ()));
                    let Ok((i, batch)) = job else {
                        return;
                    };
                    let bytes = batch.get_array_memory_size();
                    if done
                        .send((i, split(sorter, cuts, count, &batch), bytes))
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
        drop(done.take());
        let mut filler = Filler {
            buckets: &mut buckets,
            schema,
            flush_bytes: plan.flush_bytes,
            early: BTreeMap::new(),
            next: 0,
            held: 0,
        };
        let mut sent = 0;
        let mut start = 0;
        let mut read_all = || -> Result<()> {
            while start < num_rows {
                let end = (start + batch_rows).min(num_rows);
                let batch = read(start..end)?;
                jobs.as_ref()
                    .and_then(|jobs| jobs.send((sent, batch)).ok())
                    .ok_or_else(|| Error::InvalidCommand("sorting failed".into()))?;
                sent += 1;
                start = end;
                while let Ok(result) = results.try_recv() {
                    filler.add(result)?;
                }
                if !progress(start as f32 / num_rows.max(1) as f32) {
                    return Err(Error::Cancelled);
                }
            }
            Ok(())
        };
        let read_result = read_all();
        // Ends the workers, also after an error.
        drop(jobs.take());
        read_result?;
        for result in results.iter() {
            filler.add(result)?;
        }
        if filler.next < sent {
            return Err(Error::InvalidCommand("sorting failed".into()));
        }
        Ok(())
    })?;
    Ok(buckets)
}

/// Adds split batches to the buckets in input order.
struct Filler<'a> {
    buckets: &'a mut [Bucket],
    schema: &'a SchemaRef,
    flush_bytes: Option<usize>,
    /// Split batches that came back before an earlier one.
    early: BTreeMap<usize, (Vec<(usize, RecordBatch)>, usize)>,
    next: usize,
    /// Bytes held in memory since the last flush.
    held: usize,
}

impl Filler<'_> {
    fn add(
        &mut self,
        (i, split, bytes): (usize, Result<Vec<(usize, RecordBatch)>>, usize),
    ) -> Result<()> {
        self.early.insert(i, (split?, bytes));
        while let Some((parts, bytes)) = self.early.remove(&self.next) {
            for (b, rows) in parts {
                self.buckets[b].memory.push(rows);
            }
            self.held += bytes;
            self.next += 1;
            if self.flush_bytes.is_some_and(|limit| self.held > limit) {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Writes each bucket's rows in memory to a new part file, on all cores.
    fn flush(&mut self) -> Result<()> {
        let work: Vec<(usize, Vec<RecordBatch>)> = self
            .buckets
            .iter_mut()
            .enumerate()
            .filter(|(_, b)| !b.memory.is_empty())
            .map(|(i, b)| (i, std::mem::take(&mut b.memory)))
            .collect();
        let files = crate::parallel::map(work.len(), |k| {
            let file = TempFile::new()?;
            write_part(&file, self.schema, &work[k].1)?;
            Ok(file)
        })?;
        for ((b, _), file) in work.into_iter().zip(files) {
            self.buckets[b].parts.push(file);
        }
        self.held = 0;
        Ok(())
    }
}

/// The sorted rows, in order. Buckets are sorted on worker threads a few
/// ahead of the one being read; they stop when this is dropped.
pub(crate) struct Sorted {
    results: Receiver<(usize, Result<Vec<RecordBatch>>)>,
    early: BTreeMap<usize, Result<Vec<RecordBatch>>>,
    next: usize,
    count: usize,
    ready: VecDeque<RecordBatch>,
    gate: Arc<Gate>,
}

impl Sorted {
    fn start(sorter: Arc<Sorter>, schema: SchemaRef, buckets: Vec<Bucket>) -> Self {
        let count = buckets.len();
        let threads = crate::parallel::threads();
        let buckets: Arc<Vec<Mutex<Option<Bucket>>>> =
            Arc::new(buckets.into_iter().map(|b| Mutex::new(Some(b))).collect());
        let gate = Arc::new(Gate::default());
        let next = Arc::new(AtomicUsize::new(0));
        let (send, results) = channel();
        for _ in 0..threads.min(count) {
            let (buckets, gate, next, send) =
                (buckets.clone(), gate.clone(), next.clone(), send.clone());
            let (sorter, schema) = (sorter.clone(), schema.clone());
            std::thread::spawn(move || {
                loop {
                    let b = next.fetch_add(1, Ordering::Relaxed);
                    if b >= count || !gate.wait_for(b, threads) {
                        return;
                    }
                    let bucket = buckets[b].lock().ok().and_then(|mut slot| slot.take());
                    // A panic must still answer, or the reader would wait.
                    let sorted = bucket
                        .and_then(|bucket| {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                sort_bucket(&sorter, &schema, bucket)
                            }))
                            .ok()
                        })
                        .unwrap_or_else(|| Err(Error::InvalidCommand("sorting failed".into())));
                    if send.send((b, sorted)).is_err() {
                        return;
                    }
                }
            });
        }
        Self {
            results,
            early: BTreeMap::new(),
            next: 0,
            count,
            ready: VecDeque::new(),
            gate,
        }
    }
}

impl Iterator for Sorted {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Result<RecordBatch>> {
        loop {
            if let Some(batch) = self.ready.pop_front() {
                return Some(Ok(batch));
            }
            if self.next >= self.count {
                return None;
            }
            let sorted = loop {
                if let Some(sorted) = self.early.remove(&self.next) {
                    break sorted;
                }
                match self.results.recv() {
                    Ok((b, sorted)) => {
                        self.early.insert(b, sorted);
                    }
                    Err(_) => break Err(Error::InvalidCommand("sorting failed".into())),
                }
            };
            self.next += 1;
            self.gate.take(self.next);
            match sorted {
                Ok(batches) => self.ready.extend(batches),
                Err(e) => {
                    // Nothing more comes after an error.
                    self.next = self.count;
                    return Some(Err(e));
                }
            }
        }
    }
}

impl Drop for Sorted {
    fn drop(&mut self) {
        self.gate.stop.store(true, Ordering::Relaxed);
        self.gate.moved.notify_all();
    }
}

/// Lets workers sort at most a set number of buckets past the last one
/// read.
#[derive(Default)]
struct Gate {
    taken: Mutex<usize>,
    moved: Condvar,
    stop: AtomicBool,
}

impl Gate {
    /// Waits until bucket `b` may be sorted; `false` when stopped.
    fn wait_for(&self, b: usize, ahead: usize) -> bool {
        let mut taken = self.taken.lock().unwrap_or_else(|e| e.into_inner());
        while b >= *taken + ahead && !self.stop.load(Ordering::Relaxed) {
            taken = self.moved.wait(taken).unwrap_or_else(|e| e.into_inner());
        }
        !self.stop.load(Ordering::Relaxed)
    }

    fn take(&self, count: usize) {
        *self.taken.lock().unwrap_or_else(|e| e.into_inner()) = count;
        self.moved.notify_all();
    }
}

/// One bucket's rows, sorted, in batches of at most [`OUTPUT_ROWS`].
fn sort_bucket(sorter: &Sorter, schema: &SchemaRef, bucket: Bucket) -> Result<Vec<RecordBatch>> {
    let mut batches = Vec::new();
    for part in &bucket.parts {
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(part.path())?)?
            .with_batch_size(OUTPUT_ROWS)
            .build()?;
        for batch in reader {
            // The part's schema may differ in metadata.
            batches.push(RecordBatch::try_new(
                schema.clone(),
                batch?.columns().to_vec(),
            )?);
        }
    }
    batches.extend(bucket.memory);
    if bucket.single_key || batches.is_empty() {
        return Ok(batches);
    }
    let keys = batches
        .iter()
        .map(|b| sorter.rows(b))
        .collect::<Result<Vec<Rows>>>()?;
    let order = sorted_order(&keys);
    drop(keys);
    let mut starts = Vec::with_capacity(batches.len());
    let mut next = 0;
    for b in &batches {
        starts.push(next);
        next += b.num_rows();
    }
    order
        .chunks(OUTPUT_ROWS)
        .map(|chunk| {
            let picks: Vec<(usize, usize)> = chunk
                .iter()
                .map(|&row| {
                    let row = row as usize;
                    let batch = starts.partition_point(|&s| s <= row) - 1;
                    (batch, row - starts[batch])
                })
                .collect();
            let columns = (0..schema.fields().len())
                .map(|c| {
                    let arrays: Vec<&dyn Array> =
                        batches.iter().map(|b| b.column(c).as_ref()).collect();
                    Ok(interleave(&arrays, &picks)?)
                })
                .collect::<Result<Vec<_>>>()?;
            let options = RecordBatchOptions::new().with_row_count(Some(picks.len()));
            Ok(RecordBatch::try_new_with_options(
                schema.clone(),
                columns,
                &options,
            )?)
        })
        .collect()
}
