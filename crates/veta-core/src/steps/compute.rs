//! Full passes over a step's input, for steps that can't be evaluated one
//! window at a time (filter, sort, fill). The result is cached on the
//! pipeline so later reads are windowed again.

use crate::error::invalid;
use std::ops::Range;
use std::sync::Arc;

use arrow::record_batch::RecordBatch;

use super::Pipeline;
use super::checkpoint::{Checkpoint, TempFile, write_parquet};
use crate::error::Result;
use crate::source::{DataSource, MemorySource, SourceMode};

/// Rows read per batch during a full pass.
pub(crate) const PASS_ROWS: usize = 64 * 1024;

/// The cached result of a full pass.
#[derive(Debug)]
pub(crate) enum Cache {
    /// The input rows that survive, in order (filters).
    Rows(RowMap),
    /// The step's whole output (sort, fill).
    Data(Arc<dyn DataSource>),
    /// The input's rows with only these columns (a checked column removal).
    Project(arrow::datatypes::SchemaRef),
    /// The step was checked on every row and is read window by window
    /// (a cast that stops on invalid values); the output schema.
    Checked(arrow::datatypes::SchemaRef),
}

/// Kept input rows as sorted, disjoint ranges, with each range's position in
/// the output.
#[derive(Debug, Default)]
pub(crate) struct RowMap {
    ranges: Vec<Range<usize>>,
    /// Output row at which each range starts.
    starts: Vec<usize>,
    total: usize,
}

impl RowMap {
    /// Appends input rows `range`, which must come after all previous ones.
    pub(crate) fn push(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let len = range.len();
        match self.ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => {
                self.starts.push(self.total);
                self.ranges.push(range);
            }
        }
        self.total += len;
    }

    /// Number of kept rows.
    pub(super) fn total(&self) -> usize {
        self.total
    }

    /// Input row ranges for output rows `output`.
    pub(super) fn map(&self, output: Range<usize>) -> Vec<Range<usize>> {
        let mut result = Vec::new();
        if output.is_empty() {
            return result;
        }
        let first = self
            .starts
            .partition_point(|&s| s <= output.start)
            .saturating_sub(1);
        for (range, &start) in self.ranges.iter().zip(&self.starts).skip(first) {
            if start >= output.end {
                break;
            }
            let from = output.start.max(start) - start;
            let to = output.end.min(start + range.len()) - start;
            if from < to {
                result.push(range.start + from..range.start + to);
            }
        }
        result
    }
}

/// A full pass to run, detached from the document so it can run on another
/// thread. Produce it with [`Pipeline::compute_job`].
#[derive(Debug, Clone)]
pub struct ComputeJob {
    pub(super) pipeline: Pipeline,
    pub(super) step: usize,
    /// Identities of the steps up to `step`, to check the result still
    /// applies when it is installed.
    pub(super) ids: Vec<u64>,
}

/// The result of a [`ComputeJob`], to give back to the pipeline.
#[derive(Debug)]
pub struct Computed {
    pub(super) step: usize,
    pub(super) ids: Vec<u64>,
    pub(super) cache: Arc<Cache>,
}

impl ComputeJob {
    /// Index of the step being computed.
    pub fn step(&self) -> usize {
        self.step
    }

    /// See [`Pipeline::compute_key`].
    pub fn key(&self) -> &[u64] {
        &self.ids
    }

    /// Whether the step can instead produce its output once, in order,
    /// while it is saved ([`ComputeJob::stream`]): the last step, when its
    /// transform can (a filter, or a sort of a paged file). Saving it then
    /// skips the separate pass over the file, or the sorted copy.
    pub fn streams(&self) -> bool {
        let last = self.step + 1 == self.pipeline.steps().len();
        last && self.pipeline.steps()[self.step].with_transform(|t| t.streams(self))
    }

    /// The step's output, in order, without keeping it. Only for steps
    /// that [`stream`](ComputeJob::streams).
    pub(crate) fn stream(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Result<RecordBatch>> + Send + '_>> {
        let input_rows = self
            .pipeline
            .num_rows_at(self.step)
            .ok_or_else(|| invalid!(InputNotEvaluated))?;
        if !self.streams() {
            return Err(invalid!(NotStreamable));
        }
        self.pipeline.steps()[self.step].with_transform(|t| t.stream(self, input_rows))
    }

    /// Runs the pass. `progress` gets the fraction done (0–1) and returns
    /// `false` to cancel, which makes this return [`Error::Cancelled`].
    pub fn run(self, progress: &mut dyn FnMut(f32) -> bool) -> Result<Computed> {
        let input_rows = self
            .pipeline
            .num_rows_at(self.step)
            .ok_or_else(|| invalid!(InputNotEvaluated))?;
        let step = &self.pipeline.steps()[self.step];
        if !step.needs_full_pass() {
            return Err(invalid!(NoComputeNeeded, step = format!("{step:?}")));
        }
        let cache = step.with_transform(|t| t.compute(&self, input_rows, progress))?;
        Ok(Computed {
            step: self.step,
            ids: self.ids,
            cache: Arc::new(cache),
        })
    }

    /// The schema of the step's input.
    pub(crate) fn input_schema(&self) -> Result<arrow::datatypes::SchemaRef> {
        self.pipeline
            .schema_at(self.step)
            .ok_or_else(|| invalid!(InputNotEvaluated))
    }

    /// Rows `range` of the step's input.
    pub(crate) fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        self.pipeline.read_at(self.step, range)
    }

    /// Rows `range` of the step's input, only the columns `names`.
    pub(crate) fn read_columns(
        &self,
        range: Range<usize>,
        names: &[String],
    ) -> Result<RecordBatch> {
        self.pipeline.read_columns_at(self.step, range, names)
    }

    /// The step's `input_rows` input rows (all columns, or only `names`)
    /// in pieces mapped by `map` on all cores, in order; see
    /// [`Pipeline::read_parallel`].
    pub(crate) fn read_parallel<T: Send + 'static>(
        &self,
        input_rows: usize,
        names: Option<Vec<String>>,
        map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        self.pipeline
            .read_parallel(self.step, 0..input_rows, names, map)
    }

    /// How the document's source is held: results of paged documents go to
    /// temporary files.
    pub(crate) fn mode(&self) -> SourceMode {
        self.pipeline.source().mode()
    }

    /// Stores batches from `produce` in memory (in-memory documents) or in
    /// a temporary Parquet file (paged ones).
    pub(crate) fn materialize(
        &self,
        schema: arrow::datatypes::SchemaRef,
        produce: &mut dyn FnMut() -> Option<Result<RecordBatch>>,
    ) -> Result<Arc<dyn DataSource>> {
        match self.mode() {
            SourceMode::InMemory => {
                let batches = std::iter::from_fn(produce).collect::<Result<Vec<_>>>()?;
                Ok(Arc::new(MemorySource::new(schema, batches)?))
            }
            SourceMode::Paged => {
                let file = TempFile::new()?;
                write_parquet(&file, &schema, std::iter::from_fn(produce))?;
                Ok(Arc::new(Checkpoint::open(file, schema)?))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn row_map() {
        let mut map = RowMap::default();
        map.push(2..4);
        map.push(4..5); // merges
        map.push(8..10);
        map.push(20..21);
        // input: [2 3 4] [8 9] [20] → output 0..6
        assert_eq!(map.total(), 6);
        assert_eq!(map.map(0..6), vec![2..5, 8..10, 20..21]);
        assert_eq!(map.map(1..4), vec![3..5, 8..9]);
        assert_eq!(map.map(5..6), vec![20..21]);
        assert!(map.map(3..3).is_empty());
    }
}
