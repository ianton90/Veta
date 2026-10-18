//! [`Pipeline`]: a source plus its steps, evaluated lazily.

use std::ops::Range;
use std::sync::Arc;

use arrow::array::{ArrayRef, new_null_array};
use arrow::compute::concat_batches;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::compute::{Cache, ComputeJob, Computed, PASS_ROWS};
use super::edits::CellEdits;
use super::level::Level;
use super::rows::kept_ranges;
use super::{Step, invalid};
use crate::error::Result;
use crate::source::DataSource;
use crate::transforms::text::map_columns;

/// Rows per piece of a full pass read on several threads
/// ([`Pipeline::read_parallel`]).
pub(crate) const UNIT_ROWS: usize = 8 * PASS_ROWS;

/// Whether the steps' output can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Every step is evaluated.
    Ready,
    /// Step `step` needs a full pass over its input, which is running.
    /// Steps before it are evaluated.
    Computing { step: usize },
    /// Step `step` no longer applies (e.g. it refers to a column an earlier
    /// step removed). Steps before it are evaluated; it and later steps are
    /// not.
    Broken { step: usize, error: String },
}

/// A source plus its steps, evaluated lazily.
///
/// Steps are evaluated in order until one fails or needs computing; the
/// output is that of the last evaluated step (see [`Pipeline::status`]).
#[derive(Debug, Clone)]
pub struct Pipeline {
    source: Arc<dyn DataSource>,
    steps: Vec<Step>,
    /// Identity of each step's current content, to tell whether a computed
    /// result still matches the steps it was computed for.
    ids: Vec<u64>,
    next_id: u64,
    /// Computed results of full-pass steps, by step index.
    caches: Vec<Option<Arc<Cache>>>,
    /// `levels[0]` is the source; `levels[i + 1]` is the output of
    /// `steps[i]`, for the steps that are evaluated.
    levels: Vec<Level>,
    status: Status,
}

impl Pipeline {
    pub fn new(source: Arc<dyn DataSource>) -> Self {
        let level = Level {
            schema: source.schema(),
            num_rows: source.num_rows(),
        };
        Self {
            source,
            steps: Vec::new(),
            ids: Vec::new(),
            next_id: 0,
            caches: Vec::new(),
            levels: vec![level],
            status: Status::Ready,
        }
    }

    pub fn source(&self) -> &Arc<dyn DataSource> {
        &self.source
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Number of steps whose output is available (all of them when ready).
    pub fn evaluated(&self) -> usize {
        self.levels.len() - 1
    }

    /// Schema of the last evaluated step's output.
    pub fn schema(&self) -> SchemaRef {
        self.last().schema.clone()
    }

    /// Row count of the last evaluated step's output.
    pub fn num_rows(&self) -> usize {
        self.last().num_rows
    }

    /// Schema after the first `steps` steps (0 = the source). `None` if those
    /// steps are not evaluated.
    pub fn schema_at(&self, steps: usize) -> Option<SchemaRef> {
        self.levels.get(steps).map(|l| l.schema.clone())
    }

    pub fn num_rows_at(&self, steps: usize) -> Option<usize> {
        self.levels.get(steps).map(|l| l.num_rows)
    }

    fn last(&self) -> &Level {
        // There is always at least the source level.
        &self.levels[self.levels.len() - 1]
    }

    fn require_ready(&self) -> Result<()> {
        match &self.status {
            Status::Ready => Ok(()),
            Status::Computing { step } => Err(invalid(format!(
                "step {} is still being computed",
                step + 1
            ))),
            Status::Broken { step, .. } => Err(invalid(format!(
                "step {} has an error; fix or remove it first",
                step + 1
            ))),
        }
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Appends a step after validating it against the current output. Only
    /// allowed when every step is evaluated.
    pub fn push(&mut self, step: Step) -> Result<()> {
        self.require_ready()?;
        let id = self.new_id();
        if step.needs_full_pass() {
            self.last().check(&step)?;
            self.status = Status::Computing {
                step: self.steps.len(),
            };
        } else {
            let level = self.last().next(&step)?;
            self.levels.push(level);
        }
        self.steps.push(step);
        self.ids.push(id);
        self.caches.push(None);
        Ok(())
    }

    /// Removes and returns the last step.
    pub fn pop(&mut self) -> Option<Step> {
        let step = self.steps.pop()?;
        self.ids.pop();
        self.caches.pop();
        let n = self.steps.len();
        self.levels.truncate(n + 1);
        self.rebuild_from(self.levels.len() - 1);
        Some(step)
    }

    /// Replaces all steps. The first `keep` steps must be unchanged; later
    /// ones are re-evaluated, and may end up broken.
    pub fn set_steps(&mut self, steps: Vec<Step>, keep: usize) {
        let keep = keep.min(steps.len()).min(self.steps.len());
        self.ids.truncate(keep);
        for _ in keep..steps.len() {
            let id = self.new_id();
            self.ids.push(id);
        }
        self.caches.truncate(keep);
        self.caches.resize(steps.len(), None);
        self.steps = steps;
        self.levels.truncate((keep + 1).min(self.levels.len()));
        self.rebuild_from(self.levels.len() - 1);
    }

    /// Evaluates steps from `from` (all before it are evaluated) until one
    /// fails.
    fn rebuild_from(&mut self, from: usize) {
        self.levels.truncate(from + 1);
        self.status = Status::Ready;
        for (i, step) in self.steps.iter().enumerate().skip(from) {
            let result = if step.needs_full_pass() {
                match &self.caches[i] {
                    Some(cache) => Ok(self.levels[i].after_pass(cache)),
                    None => match self.levels[i].check(step) {
                        Ok(()) => {
                            self.status = Status::Computing { step: i };
                            return;
                        }
                        Err(e) => Err(e),
                    },
                }
            } else {
                self.levels[i].next(step)
            };
            match result {
                Ok(level) => self.levels.push(level),
                Err(e) => {
                    self.status = Status::Broken {
                        step: i,
                        error: e.to_string(),
                    };
                    return;
                }
            }
        }
    }

    /// The full pass to run next, if a step is waiting to be computed.
    pub fn compute_job(&self) -> Option<ComputeJob> {
        let Status::Computing { step } = self.status else {
            return None;
        };
        Some(ComputeJob {
            pipeline: self.clone(),
            step,
            ids: self.ids[..=step].to_vec(),
        })
    }

    /// Identifies the pending full pass: the ids of the steps up to and
    /// including it. Changes whenever a different job would be needed.
    pub fn compute_key(&self) -> Option<Vec<u64>> {
        let Status::Computing { step } = self.status else {
            return None;
        };
        Some(self.ids[..=step].to_vec())
    }

    /// Marks the pending step broken after its pass failed. Returns `false`
    /// (and does nothing) if `key` is stale.
    pub fn fail(&mut self, key: &[u64], error: String) -> bool {
        if self.compute_key().as_deref() != Some(key) {
            return false;
        }
        self.status = Status::Broken {
            step: key.len() - 1,
            error,
        };
        true
    }

    /// Stores a computed result and evaluates the following steps. Returns
    /// `false` (and does nothing) if the steps changed since the job was
    /// created.
    pub fn install(&mut self, computed: Computed) -> bool {
        let current = matches!(self.status, Status::Computing { step } if step == computed.step)
            && self.ids.get(..=computed.step) == Some(computed.ids.as_slice());
        if !current {
            return false;
        }
        self.caches[computed.step] = Some(computed.cache);
        self.rebuild_from(computed.step);
        true
    }

    /// Runs every pending full pass on this thread.
    pub fn compute_all(&mut self, progress: &mut dyn FnMut(f32) -> bool) -> Result<()> {
        self.compute_until(false, progress).map(|_| ())
    }

    /// Runs the pending full passes before saving, except a last one that
    /// [`streams`](ComputeJob::streams) into the file: that one is returned.
    pub(crate) fn compute_for_save(
        &mut self,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Option<ComputeJob>> {
        self.compute_until(true, progress)
    }

    fn compute_until(
        &mut self,
        stop_at_stream: bool,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Option<ComputeJob>> {
        while let Some(job) = self.compute_job() {
            if stop_at_stream && job.streams() {
                return Ok(Some(job));
            }
            let key = job.ids.clone();
            match job.run(progress) {
                Ok(computed) => {
                    self.install(computed);
                }
                Err(crate::Error::Cancelled) => return Err(crate::Error::Cancelled),
                // The step is marked broken, as when computed in the
                // background.
                Err(e) => {
                    self.fail(&key, e.to_string());
                    return Err(e);
                }
            }
        }
        Ok(None)
    }

    /// The last step's cell edits, if the last step edits cells. Callers must
    /// only store values already validated for the column and row.
    pub(crate) fn last_edits_mut(&mut self) -> Option<&mut CellEdits> {
        if self.status != Status::Ready {
            return None;
        }
        match self.steps.last_mut() {
            Some(Step::EditCells(edits)) => Some(edits),
            _ => None,
        }
    }

    /// Reads rows in `range` of the last evaluated step's output (clamped to
    /// its row count).
    pub fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        self.read_at(self.evaluated(), range)
    }

    /// Reads rows after the first `steps` steps (0 = the source), clamped to
    /// that output's row count.
    pub fn read_at(&self, steps: usize, range: Range<usize>) -> Result<RecordBatch> {
        let Some(level) = self.levels.get(steps) else {
            return Err(invalid(format!("step {steps} is not evaluated")));
        };
        let end = range.end.min(level.num_rows);
        self.read_level(steps, range.start.min(end)..end)
    }

    /// Reads rows `range` after `steps` steps (only the columns `names`, if
    /// given) in pieces of [`UNIT_ROWS`], several at once on other threads,
    /// and returns `map(batches, first row)` for each piece, in order. A
    /// piece's batches are at most [`PASS_ROWS`] rows each.
    pub(crate) fn read_parallel<T: Send + 'static>(
        &self,
        steps: usize,
        range: Range<usize>,
        names: Option<Vec<String>>,
        map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        let pipeline = self.clone();
        let pieces = range.len().div_ceil(UNIT_ROWS);
        crate::parallel::ordered(pieces, crate::parallel::threads(), move |piece| {
            let first = range.start + piece * UNIT_ROWS;
            let end = (first + UNIT_ROWS).min(range.end);
            let batches = (first..end)
                .step_by(PASS_ROWS)
                .map(|start| {
                    let window = start..(start + PASS_ROWS).min(end);
                    match &names {
                        Some(names) => pipeline.read_columns_at(steps, window, names),
                        None => pipeline.read_at(steps, window),
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            map(batches, first)
        })
    }

    /// Like [`Self::read_at`], with only the columns named in `names` (in
    /// the level's order). Reads only those columns from the file where it
    /// can (straight from the source, or from a computed step's data).
    pub fn read_columns_at(
        &self,
        steps: usize,
        range: Range<usize>,
        names: &[String],
    ) -> Result<RecordBatch> {
        let Some(level) = self.levels.get(steps) else {
            return Err(invalid(format!("step {steps} is not evaluated")));
        };
        let end = range.end.min(level.num_rows);
        let range = range.start.min(end)..end;
        if steps == 0 {
            return self.source.read_columns(range, names);
        }
        if let Some(cache) = &self.caches[steps - 1]
            && let Cache::Data(source) = cache.as_ref()
        {
            return source.read_columns(range, names);
        }
        crate::source::project(&self.read_level(steps, range)?, names)
    }

    /// Reads rows of level `level`; `range` must be within it.
    fn read_level(&self, level: usize, range: Range<usize>) -> Result<RecordBatch> {
        if level == 0 {
            return self.source.read(range);
        }
        let schema = self.levels[level].schema.clone();
        let previous = level - 1;
        if let Some(cache) = &self.caches[previous]
            && !matches!(cache.as_ref(), Cache::Checked(_))
        {
            return match cache.as_ref() {
                Cache::Rows(rows) => {
                    // Kept rows close together are read as one span and
                    // picked out with a mask: a filter keeping scattered rows
                    // has millions of short runs, too many to read one by one.
                    let parts = span_groups(&rows.map(range))
                        .into_iter()
                        .map(|(span, runs)| {
                            let batch = self.read_level(previous, span.clone())?;
                            if runs.len() == 1 && runs[0] == span {
                                return Ok(batch);
                            }
                            let mut mask = arrow::array::BooleanBufferBuilder::new(span.len());
                            mask.append_n(span.len(), false);
                            for run in runs {
                                for i in run.start - span.start..run.end - span.start {
                                    mask.set_bit(i, true);
                                }
                            }
                            let mask = arrow::array::BooleanArray::new(mask.finish(), None);
                            Ok(arrow::compute::filter_record_batch(&batch, &mask)?)
                        })
                        .collect::<Result<Vec<_>>>()?;
                    concat(&schema, parts)
                }
                Cache::Data(source) => source.read(range),
                Cache::Project(_) => {
                    let batch = self.read_level(previous, range)?;
                    let input = batch.schema();
                    let columns = schema
                        .fields()
                        .iter()
                        .map(|f| Ok(batch.column(input.index_of(f.name())?).clone()))
                        .collect::<Result<Vec<_>>>()?;
                    with_schema(schema, columns, batch.num_rows())
                }
                // Read window by window below.
                Cache::Checked(_) => unreachable!("handled above"),
            };
        }
        match &self.steps[previous] {
            Step::EditCells(edits) => {
                edits.apply(&self.read_level(previous, range.clone())?, range.start)
            }
            Step::InsertRows { at, count } => {
                let (at, count) = (*at, *count);
                let mut parts = Vec::new();
                let before = range.start..range.end.min(at);
                if !before.is_empty() {
                    parts.push(self.read_level(previous, before)?);
                }
                let inserted = range.start.max(at)..range.end.min(at + count);
                if !inserted.is_empty() {
                    parts.push(empty_rows(&schema, inserted.len())?);
                }
                let after = range.start.max(at + count)..range.end;
                if !after.is_empty() {
                    parts.push(self.read_level(previous, after.start - count..after.end - count)?);
                }
                concat(&schema, parts)
            }
            Step::DeleteRows { rows } => {
                let parts = kept_ranges(rows, range)
                    .into_iter()
                    .map(|r| self.read_level(previous, r))
                    .collect::<Result<Vec<_>>>()?;
                concat(&schema, parts)
            }
            Step::AddColumn { name, .. } => {
                let batch = self.read_level(previous, range)?;
                let index = schema.index_of(name)?;
                let mut columns = batch.columns().to_vec();
                columns.insert(
                    index,
                    new_null_array(schema.field(index).data_type(), batch.num_rows()),
                );
                with_schema(schema, columns, batch.num_rows())
            }
            // Full-pass steps are read through their cache above.
            Step::Cast(cast) => cast.apply(&self.read_level(previous, range)?, &schema),
            Step::Split(split) => split.apply(&self.read_level(previous, range)?, &schema),
            Step::CustomColumn(custom) => custom.apply(
                &self.read_level(previous, range.clone())?,
                range.start,
                &schema,
            ),
            Step::Merge(merge) => merge.apply(&self.read_level(previous, range)?, &schema),
            Step::AddField { column, .. } | Step::RemoveField { column, .. } => {
                crate::transforms::nested::change_field(
                    &self.read_level(previous, range)?,
                    column,
                    &schema,
                )
            }
            Step::CombineStruct { columns, name } => crate::transforms::nested::combine(
                &self.read_level(previous, range)?,
                columns,
                name,
                &schema,
            ),
            Step::Replace(replace) => map_columns(
                &self.read_level(previous, range)?,
                &schema,
                std::slice::from_ref(&replace.column),
                |c| replace.apply(c),
            ),
            Step::FormatText { columns, op } => {
                map_columns(&self.read_level(previous, range)?, &schema, columns, |c| {
                    op.apply(c)
                })
            }
            Step::ExpandStruct { column, all_levels } => {
                let input = self.read_level(previous, range)?;
                crate::transforms::nested::expand_levels(&input, column, *all_levels, &schema)
            }
            Step::Filter(_)
            | Step::Sort(_)
            | Step::Fill(_)
            | Step::RemoveEmptyColumn { .. }
            | Step::ExplodeList { .. }
            | Step::CollectList { .. } => Err(invalid("this step must be computed first".into())),
            Step::RemoveColumns { .. }
            | Step::KeepColumns { .. }
            | Step::RenameColumn { .. }
            | Step::MoveColumn { .. } => {
                // Pick the previous columns by name in the new order; renames
                // map the new name back to the old one.
                let batch = self.read_level(previous, range)?;
                let prev_schema = &self.levels[previous].schema;
                let columns = schema
                    .fields()
                    .iter()
                    .map(|f| {
                        let name = match &self.steps[previous] {
                            Step::RenameColumn { from, to } if f.name() == to => from,
                            _ => f.name(),
                        };
                        Ok(batch.column(prev_schema.index_of(name)?).clone())
                    })
                    .collect::<Result<Vec<_>>>()?;
                with_schema(schema, columns, batch.num_rows())
            }
        }
    }
}

/// Rows skipped between runs that are still read as one span (see
/// [`span_groups`]).
const SPAN_GAP: usize = 4096;

/// Groups input row runs (in order) into spans to read at once: runs less
/// than [`SPAN_GAP`] rows apart share a span. Returns each span with its runs.
fn span_groups(runs: &[Range<usize>]) -> Vec<(Range<usize>, Vec<Range<usize>>)> {
    let mut groups: Vec<(Range<usize>, Vec<Range<usize>>)> = Vec::new();
    for run in runs {
        match groups.last_mut() {
            Some((span, members)) if run.start <= span.end + SPAN_GAP => {
                span.end = run.end;
                members.push(run.clone());
            }
            _ => groups.push((run.clone(), vec![run.clone()])),
        }
    }
    groups
}

/// A batch of `rows` rows with every value null.
fn empty_rows(schema: &SchemaRef, rows: usize) -> Result<RecordBatch> {
    let columns = schema
        .fields()
        .iter()
        .map(|f| new_null_array(f.data_type(), rows))
        .collect();
    with_schema(schema.clone(), columns, rows)
}

fn with_schema(schema: SchemaRef, columns: Vec<ArrayRef>, rows: usize) -> Result<RecordBatch> {
    let options = RecordBatchOptions::new().with_row_count(Some(rows));
    Ok(RecordBatch::try_new_with_options(
        schema, columns, &options,
    )?)
}

/// Concatenates parts, giving them `schema` (parts may differ only in
/// nullability).
fn concat(schema: &SchemaRef, parts: Vec<RecordBatch>) -> Result<RecordBatch> {
    let parts = parts
        .into_iter()
        .map(|b| {
            let rows = b.num_rows();
            with_schema(schema.clone(), b.columns().to_vec(), rows)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(concat_batches(schema, &parts)?)
}

#[cfg(test)]
mod span_tests {
    use super::*;

    #[test]
    fn runs_close_together_share_a_span() {
        let runs = [0..2, 5..6, 4000..4001, 20_000..20_010, 20_011..20_012];
        let groups = span_groups(&runs);
        let spans: Vec<_> = groups.iter().map(|(s, _)| s.clone()).collect();
        assert_eq!(spans, [0..4001, 20_000..20_012]);
        assert_eq!(groups[0].1, [0..2, 5..6, 4000..4001]);
        assert!(span_groups(&[]).is_empty());
    }
}
