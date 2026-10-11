//! Full passes over a step's input, for steps that can't be evaluated one
//! window at a time (filter, sort, fill). The result is cached on the
//! pipeline so later reads are windowed again.

use std::ops::Range;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef};
use arrow::record_batch::RecordBatch;

use super::checkpoint::{Checkpoint, TempFile, write_parquet};
use super::fill::{Fill, carry_out, fill};
use super::sort::{sort_external, sort_in_memory};
use super::{Pipeline, Step, invalid};
use crate::error::{Error, Result};
use crate::source::{DataSource, MemorySource, SourceMode};

/// Rows read per batch during a full pass.
pub(super) const PASS_ROWS: usize = 64 * 1024;

/// The cached result of a full pass.
#[derive(Debug)]
pub(crate) enum Cache {
    /// The input rows that survive, in order (filters).
    Rows(RowMap),
    /// The step's whole output (sort, fill).
    Data(Arc<dyn DataSource>),
    /// The input's rows with only these columns (a checked column removal).
    Project(arrow::datatypes::SchemaRef),
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
    pub(super) fn push(&mut self, range: Range<usize>) {
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

    /// Runs the pass. `progress` gets the fraction done (0–1) and returns
    /// `false` to cancel, which makes this return [`Error::Cancelled`].
    pub fn run(self, progress: &mut dyn FnMut(f32) -> bool) -> Result<Computed> {
        let input_rows = self
            .pipeline
            .num_rows_at(self.step)
            .ok_or_else(|| invalid("the step's input is not evaluated".into()))?;
        let cache = match &self.pipeline.steps()[self.step] {
            Step::Filter(filter) => {
                let mut rows = RowMap::default();
                let mut start = 0;
                while start < input_rows {
                    let end = (start + PASS_ROWS).min(input_rows);
                    let batch = self.pipeline.read_at(self.step, start..end)?;
                    let mask = filter.evaluate(&batch)?;
                    for (from, to) in mask.values().set_slices() {
                        rows.push(start + from..start + to);
                    }
                    start = end;
                    if !progress(start as f32 / input_rows as f32) {
                        return Err(Error::Cancelled);
                    }
                }
                Cache::Rows(rows)
            }
            Step::Sort(sort) => {
                let schema = self
                    .pipeline
                    .schema_at(self.step)
                    .ok_or_else(|| invalid("the step's input is not evaluated".into()))?;
                let read = |range| self.pipeline.read_at(self.step, range);
                let source: Arc<dyn DataSource> = match self.pipeline.source().mode() {
                    SourceMode::InMemory => {
                        let batch =
                            sort_in_memory(sort, &schema, input_rows, &read, PASS_ROWS, progress)?;
                        Arc::new(MemorySource::new(schema, vec![batch])?)
                    }
                    SourceMode::Paged => Arc::new(sort_external(
                        sort, &schema, input_rows, &read, PASS_ROWS, None, progress,
                    )?),
                };
                Cache::Data(source)
            }
            Step::Fill(f) => Cache::Data(self.fill(f, input_rows, progress)?),
            Step::ExplodeList { column } => {
                Cache::Data(self.explode(column, input_rows, progress)?)
            }
            Step::RemoveEmptyColumn { name } => {
                let mut values = 0;
                let mut example = None;
                let mut start = 0;
                while start < input_rows {
                    let end = (start + PASS_ROWS).min(input_rows);
                    let batch = self.pipeline.read_at(self.step, start..end)?;
                    let column = batch
                        .column_by_name(name)
                        .ok_or_else(|| invalid(format!("no column named {name:?}")))?;
                    values += column.len() - column.null_count();
                    if example.is_none()
                        && let Some(i) = (0..column.len()).find(|&i| column.is_valid(i))
                    {
                        example = arrow::util::display::array_value_to_string(column, i).ok();
                    }
                    start = end;
                    if !progress(start as f32 / input_rows as f32) {
                        return Err(Error::Cancelled);
                    }
                }
                if values > 0 {
                    let example = example.map(|e| format!(", e.g. {e:?}")).unwrap_or_default();
                    return Err(invalid(format!(
                        "{name} isn't empty: {values} values{example}"
                    )));
                }
                let schema = self
                    .pipeline
                    .schema_at(self.step)
                    .ok_or_else(|| invalid("the step's input is not evaluated".into()))?;
                let fields: Vec<_> = schema
                    .fields()
                    .iter()
                    .filter(|f| f.name() != name)
                    .map(|f| f.as_ref().clone())
                    .collect();
                Cache::Project(Arc::new(arrow::datatypes::Schema::new_with_metadata(
                    fields,
                    schema.metadata().clone(),
                )))
            }
            other => {
                return Err(invalid(format!(
                    "{} doesn't need computing",
                    other.describe()
                )));
            }
        };
        Ok(Computed {
            step: self.step,
            ids: self.ids,
            cache: Arc::new(cache),
        })
    }
}

impl ComputeJob {
    /// Fills nulls batch by batch, carrying the last value across batches.
    /// Filling up first finds, from the end, the value carried into each
    /// batch.
    fn fill(
        &self,
        f: &Fill,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Arc<dyn DataSource>> {
        let schema = self
            .pipeline
            .schema_at(self.step)
            .ok_or_else(|| invalid("the step's input is not evaluated".into()))?;
        let indices = f
            .columns
            .iter()
            .map(|c| schema.index_of(c))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let batches: Vec<Range<usize>> = (0..input_rows)
            .step_by(PASS_ROWS)
            .map(|start| start..(start + PASS_ROWS).min(input_rows))
            .collect();
        let read = |range: &Range<usize>| self.pipeline.read_at(self.step, range.clone());

        // Values carried into each batch, per filled column (filling up).
        let mut carried_up: Vec<Vec<Option<ArrayRef>>> = Vec::new();
        let first_share = if f.up { 0.5 } else { 0.0 };
        if f.up {
            let mut carry: Vec<Option<ArrayRef>> = vec![None; indices.len()];
            carried_up = vec![Vec::new(); batches.len()];
            for (done, (b, range)) in batches.iter().enumerate().rev().enumerate() {
                carried_up[b] = carry.clone();
                let batch = read(range)?;
                for (k, &i) in indices.iter().enumerate() {
                    carry[k] = carry_out(batch.column(i), carry[k].take(), true);
                }
                if !progress(first_share * (done + 1) as f32 / batches.len() as f32) {
                    return Err(Error::Cancelled);
                }
            }
        }

        let mut carry: Vec<Option<ArrayRef>> = vec![None; indices.len()];
        let mut next = 0;
        let mut produce = || -> Option<Result<RecordBatch>> {
            let range = batches.get(next)?;
            let b = next;
            next += 1;
            let result = (|| {
                let batch = read(range)?;
                let mut columns = batch.columns().to_vec();
                for (k, &i) in indices.iter().enumerate() {
                    let incoming = if f.up {
                        carried_up[b][k].clone()
                    } else {
                        carry[k].clone()
                    };
                    columns[i] = fill(&columns[i], incoming.as_ref(), f.up)?;
                    if !f.up {
                        carry[k] = carry_out(batch.column(i), carry[k].take(), false);
                    }
                }
                let fraction = (b + 1) as f32 / batches.len() as f32;
                if !progress(first_share + (1.0 - first_share) * fraction) {
                    return Err(Error::Cancelled);
                }
                Ok(RecordBatch::try_new(schema.clone(), columns)?)
            })();
            Some(result)
        };
        self.materialize(schema.clone(), &mut produce)
    }

    /// Stores batches from `produce` in memory (in-memory documents) or in
    /// a temporary Parquet file (paged ones).
    fn materialize(
        &self,
        schema: arrow::datatypes::SchemaRef,
        produce: &mut dyn FnMut() -> Option<Result<RecordBatch>>,
    ) -> Result<Arc<dyn DataSource>> {
        match self.pipeline.source().mode() {
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

    /// One row per list item, batch by batch.
    fn explode(
        &self,
        column: &str,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Arc<dyn DataSource>> {
        let input = self
            .pipeline
            .schema_at(self.step)
            .ok_or_else(|| invalid("the step's input is not evaluated".into()))?;
        let schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
            super::nested::explode_fields(&input, column)?,
            input.metadata().clone(),
        ));
        let mut start = 0;
        let mut produce = || -> Option<Result<RecordBatch>> {
            if start >= input_rows {
                return None;
            }
            let end = (start + PASS_ROWS).min(input_rows);
            let result = self
                .pipeline
                .read_at(self.step, start..end)
                .and_then(|batch| super::nested::explode(&batch, column, &schema));
            start = end;
            if result.is_ok() && !progress(start as f32 / input_rows as f32) {
                return Some(Err(Error::Cancelled));
            }
            Some(result)
        };
        self.materialize(schema.clone(), &mut produce)
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
