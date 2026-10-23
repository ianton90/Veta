//! Full passes over a step's input, for steps that can't be evaluated one
//! window at a time (filter, sort, fill). The result is cached on the
//! pipeline so later reads are windowed again.

use crate::error::invalid;
use std::ops::Range;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef};
use arrow::record_batch::RecordBatch;

use super::checkpoint::{Checkpoint, TempFile, write_parquet};
use super::{Pipeline, Step};
use crate::error::{Error, Result};
use crate::source::{DataSource, MemorySource, SourceMode};
use crate::transforms::fill::{Fill, carry_out, fill};
use crate::transforms::sort::{sort_external, sort_in_memory, sort_stream};

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

    /// Whether the step can instead produce its output once, in order,
    /// while it is saved ([`ComputeJob::stream`]): a last step that filters,
    /// or sorts a paged file. Saving it then skips the separate pass over
    /// the file, or the sorted copy.
    pub fn streams(&self) -> bool {
        let last = self.step + 1 == self.pipeline.steps().len();
        last && match &self.pipeline.steps()[self.step] {
            Step::Filter(_) => true,
            Step::Sort(_) => self.pipeline.source().mode() == SourceMode::Paged,
            _ => false,
        }
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
        match &self.pipeline.steps()[self.step] {
            Step::Filter(filter) => {
                // Pieces are read and filtered on several threads.
                let filter = filter.clone();
                let pieces = self.pipeline.read_parallel(
                    self.step,
                    0..input_rows,
                    None,
                    move |batches, _| {
                        batches
                            .iter()
                            .map(|batch| {
                                let mask = filter.evaluate(batch)?;
                                Ok(arrow::compute::filter_record_batch(batch, &mask)?)
                            })
                            .collect::<Result<Vec<_>>>()
                    },
                );
                Ok(Box::new(pieces.flat_map(|piece| match piece {
                    Ok(batches) => batches.into_iter().map(Ok).collect(),
                    Err(e) => vec![Err(e)],
                })))
            }
            Step::Sort(sort) if self.streams() => {
                let schema = self
                    .pipeline
                    .schema_at(self.step)
                    .ok_or_else(|| invalid!(InputNotEvaluated))?;
                let read = |range| self.pipeline.read_at(self.step, range);
                let names = sort_key_names(sort);
                let read_keys = |range| self.pipeline.read_columns_at(self.step, range, &names);
                Ok(Box::new(sort_stream(
                    sort, &schema, input_rows, &read, &read_keys, PASS_ROWS,
                )?))
            }
            _ => Err(invalid!(NotStreamable)),
        }
    }

    /// Runs the pass. `progress` gets the fraction done (0–1) and returns
    /// `false` to cancel, which makes this return [`Error::Cancelled`].
    pub fn run(self, progress: &mut dyn FnMut(f32) -> bool) -> Result<Computed> {
        let input_rows = self
            .pipeline
            .num_rows_at(self.step)
            .ok_or_else(|| invalid!(InputNotEvaluated))?;
        let cache = match &self.pipeline.steps()[self.step] {
            Step::Filter(filter) => {
                // Only the columns the conditions test are read.
                let mut names: Vec<String> =
                    filter.conditions.iter().map(|c| c.column.clone()).collect();
                names.dedup();
                // Pieces are read and tested on several threads; the kept
                // ranges are joined in order.
                let tested = filter.clone();
                let pieces = self.pipeline.read_parallel(
                    self.step,
                    0..input_rows,
                    Some(names),
                    move |batches, first| {
                        let mut kept = Vec::new();
                        let mut start = first;
                        for batch in &batches {
                            let mask = tested.evaluate(batch)?;
                            for (from, to) in mask.values().set_slices() {
                                kept.push(start + from..start + to);
                            }
                            start += batch.num_rows();
                        }
                        Ok((kept, start))
                    },
                );
                let mut rows = RowMap::default();
                for piece in pieces {
                    let (kept, end) = piece?;
                    for range in kept {
                        rows.push(range);
                    }
                    if !progress(end as f32 / input_rows as f32) {
                        return Err(Error::Cancelled);
                    }
                }
                Cache::Rows(rows)
            }
            Step::Sort(sort) => {
                let schema = self
                    .pipeline
                    .schema_at(self.step)
                    .ok_or_else(|| invalid!(InputNotEvaluated))?;
                let read = |range| self.pipeline.read_at(self.step, range);
                let source: Arc<dyn DataSource> = match self.pipeline.source().mode() {
                    SourceMode::InMemory => {
                        let batch =
                            sort_in_memory(sort, &schema, input_rows, &read, PASS_ROWS, progress)?;
                        Arc::new(MemorySource::new(schema, vec![batch])?)
                    }
                    SourceMode::Paged => {
                        let names = sort_key_names(sort);
                        let read_keys =
                            |range| self.pipeline.read_columns_at(self.step, range, &names);
                        Arc::new(sort_external(
                            sort, &schema, input_rows, &read, &read_keys, PASS_ROWS, None, progress,
                        )?)
                    }
                };
                Cache::Data(source)
            }
            Step::Fill(f) => Cache::Data(self.fill(f, input_rows, progress)?),
            Step::ExplodeList { column } => {
                Cache::Data(self.explode(column, input_rows, progress)?)
            }
            Step::CollectList { keys, columns } => {
                Cache::Data(self.collect(keys, columns, input_rows, progress)?)
            }
            Step::Cast(cast) => {
                // Converts every row once so an invalid value breaks the
                // step instead of failing later reads.
                let input = self
                    .pipeline
                    .schema_at(self.step)
                    .ok_or_else(|| invalid!(InputNotEvaluated))?;
                let schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
                    cast.output_fields(&input)?,
                    input.metadata().clone(),
                ));
                let mut start = 0;
                while start < input_rows {
                    let end = (start + PASS_ROWS).min(input_rows);
                    let batch = self.pipeline.read_at(self.step, start..end)?;
                    if let Some((row, value)) = cast.first_failure(&batch)? {
                        return Err(invalid!(
                            RowValueType,
                            row = start + row + 1,
                            value = value,
                            column = cast.column,
                            data_type = crate::display::type_name(&cast.to)
                        ));
                    }
                    start = end;
                    if !progress(start as f32 / input_rows as f32) {
                        return Err(Error::Cancelled);
                    }
                }
                Cache::Checked(schema)
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
                        .ok_or_else(|| invalid!(ColumnNotFound, column = name))?;
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
                    return Err(match example {
                        Some(example) => invalid!(
                            NotEmptyExample,
                            column = name,
                            values = values,
                            example = example
                        ),
                        None => invalid!(NotEmpty, column = name, values = values),
                    });
                }
                let schema = self
                    .pipeline
                    .schema_at(self.step)
                    .ok_or_else(|| invalid!(InputNotEvaluated))?;
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
                return Err(invalid!(NoComputeNeeded, step = format!("{other:?}")));
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
            .ok_or_else(|| invalid!(InputNotEvaluated))?;
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

    /// One row per group of equal keys, the other columns collected into
    /// lists. Rows are sorted by the keys first (stable, so items keep their
    /// order), which also orders the groups.
    fn collect(
        &self,
        keys: &[String],
        columns: &[String],
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Arc<dyn DataSource>> {
        use crate::transforms::sort::{Sort, SortKey};
        let input = self
            .pipeline
            .schema_at(self.step)
            .ok_or_else(|| invalid!(InputNotEvaluated))?;
        let schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
            crate::transforms::nested::collect_fields(&input, keys, columns)?,
            input.metadata().clone(),
        ));
        let read = |range| self.pipeline.read_at(self.step, range);
        let sorted: Option<Arc<dyn DataSource>> = if keys.is_empty() {
            None
        } else {
            let sort = Sort {
                keys: keys
                    .iter()
                    .map(|k| SortKey {
                        column: k.clone(),
                        descending: false,
                        nulls_first: false,
                    })
                    .collect(),
            };
            let mut sort_progress = |f: f32| progress(0.5 * f);
            Some(match self.pipeline.source().mode() {
                SourceMode::InMemory => {
                    let batch = sort_in_memory(
                        &sort,
                        &input,
                        input_rows,
                        &read,
                        PASS_ROWS,
                        &mut sort_progress,
                    )?;
                    Arc::new(MemorySource::new(input.clone(), vec![batch])?)
                }
                SourceMode::Paged => Arc::new(sort_external(
                    &sort,
                    &input,
                    input_rows,
                    &read,
                    &|range| self.pipeline.read_columns_at(self.step, range, keys),
                    PASS_ROWS,
                    None,
                    &mut sort_progress,
                )?),
            })
        };
        let read_sorted = |range: Range<usize>| match &sorted {
            Some(source) => source.read(range),
            None => read(range),
        };
        let key_columns = keys
            .iter()
            .map(|k| input.index_of(k))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let converter = if keys.is_empty() {
            None
        } else {
            let fields = key_columns
                .iter()
                .map(|&i| arrow::row::SortField::new(input.field(i).data_type().clone()))
                .collect();
            Some(arrow::row::RowConverter::new(fields)?)
        };

        // Groups can span reads: the rows of the last, maybe unfinished,
        // group wait in `pending` (as read, joined once the group ends) for
        // the next read. Without keys the whole input is one group.
        let mut start = 0;
        let mut pending: Vec<RecordBatch> = Vec::new();
        // Whether row `b` of `next` has the same keys as the last pending row.
        let continues = |pending: &[RecordBatch], next: &RecordBatch| -> Result<bool> {
            let (Some(last), Some(converter)) = (pending.last(), converter.as_ref()) else {
                return Ok(!pending.is_empty());
            };
            let keys = |batch: &RecordBatch, row: usize| -> Vec<arrow::array::ArrayRef> {
                key_columns
                    .iter()
                    .map(|&i| batch.column(i).slice(row, 1))
                    .collect()
            };
            let a = converter.convert_columns(&keys(last, last.num_rows() - 1))?;
            let b = converter.convert_columns(&keys(next, 0))?;
            Ok(a.row(0) == b.row(0))
        };
        let mut next = |progress: &mut dyn FnMut(f32) -> bool| -> Result<Option<RecordBatch>> {
            loop {
                let end = (start + PASS_ROWS).min(input_rows);
                let read = (start < end).then(|| read_sorted(start..end)).transpose()?;
                start = end;
                let last = start >= input_rows;
                if !progress(0.5 + 0.5 * start as f32 / input_rows.max(1) as f32) {
                    return Err(Error::Cancelled);
                }
                let read = match read {
                    Some(read) if read.num_rows() > 0 => read,
                    _ if pending.is_empty() => return Ok(None),
                    // The end: the pending group is complete.
                    _ => {
                        let rows: usize = pending.iter().map(RecordBatch::num_rows).sum();
                        let group = arrow::compute::concat_batches(&input, &pending)?;
                        pending.clear();
                        return group_lists(&group, &[0, rows], keys, &schema).map(Some);
                    }
                };
                let starts = crate::transforms::nested::group_starts(
                    &read,
                    &key_columns,
                    converter.as_ref(),
                )?;
                let joins = continues(&pending, &read)?;
                if joins && starts.len() == 1 && !last {
                    // All of it belongs to the pending group.
                    pending.push(read);
                    continue;
                }
                // Complete: the pending group and this read's groups up to its
                // last one (which may go on in the next read).
                let cut = match last {
                    true => read.num_rows(),
                    false => *starts.last().expect("rows read"),
                };
                let offset: usize = pending.iter().map(RecordBatch::num_rows).sum();
                let mut bounds: Vec<usize> = if pending.is_empty() { vec![] } else { vec![0] };
                bounds.extend(
                    starts
                        .iter()
                        .filter(|&&s| s < cut && !(joins && s == 0))
                        .map(|s| offset + s),
                );
                bounds.push(offset + cut);
                pending.push(read.slice(0, cut));
                let complete = arrow::compute::concat_batches(&input, &pending)?;
                pending.clear();
                if !last {
                    pending.push(read.slice(cut, read.num_rows() - cut));
                }
                if complete.num_rows() > 0 {
                    return group_lists(&complete, &bounds, keys, &schema).map(Some);
                }
            }
        };
        let mut produce = || next(progress).transpose();
        self.materialize(schema.clone(), &mut produce)
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
            .ok_or_else(|| invalid!(InputNotEvaluated))?;
        let schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
            crate::transforms::nested::explode_fields(&input, column)?,
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
                .and_then(|batch| crate::transforms::nested::explode(&batch, column, &schema));
            start = end;
            if result.is_ok() && !progress(start as f32 / input_rows as f32) {
                return Some(Err(Error::Cancelled));
            }
            Some(result)
        };
        self.materialize(schema.clone(), &mut produce)
    }
}

/// The columns a sort reads to sample its keys.
/// [`collect_groups`](crate::transforms::nested::collect_groups), refusing
/// a group too large for one list (2^31 values).
fn group_lists(
    batch: &RecordBatch,
    bounds: &[usize],
    keys: &[String],
    schema: &Arc<arrow::datatypes::Schema>,
) -> Result<RecordBatch> {
    if batch.num_rows() > i32::MAX as usize {
        return Err(invalid!(CollectTooLarge));
    }
    crate::transforms::nested::collect_groups(batch, bounds, keys, schema)
}

fn sort_key_names(sort: &crate::transforms::sort::Sort) -> Vec<String> {
    sort.keys.iter().map(|k| k.column.clone()).collect()
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
