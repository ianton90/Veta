//! [`Pipeline`]: a source plus its steps, evaluated lazily.

use crate::error::invalid;
use std::ops::Range;
use std::sync::Arc;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

use super::Step;
use super::compute::{Cache, ComputeJob, Computed, PASS_ROWS};
use super::edits::CellEdits;
use super::level::Level;
use crate::error::{Error, Result};
use crate::source::DataSource;

/// Rows per piece of a full pass read on several threads
/// ([`Pipeline::read_parallel`]).
pub(crate) const UNIT_ROWS: usize = 16 * PASS_ROWS;

/// Whether the steps' output can be read.
#[derive(Debug, Clone)]
pub enum Status {
    /// Every step is evaluated.
    Ready,
    /// Step `step` needs a full pass over its input, which is running.
    /// Steps before it are evaluated.
    Computing { step: usize },
    /// Step `step` no longer applies (e.g. it refers to a column an earlier
    /// step removed). Steps before it are evaluated; it and later steps are
    /// not.
    Broken { step: usize, error: Arc<Error> },
}

/// Two statuses are equal when they are the same state; broken ones also
/// share the same error.
impl PartialEq for Status {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Status::Ready, Status::Ready) => true,
            (Status::Computing { step: a }, Status::Computing { step: b }) => a == b,
            (Status::Broken { step: a, error: e }, Status::Broken { step: b, error: f }) => {
                a == b && Arc::ptr_eq(e, f)
            }
            _ => false,
        }
    }
}

impl Eq for Status {}

/// A source plus its steps, evaluated lazily.
///
/// Steps are evaluated in order until one fails or needs computing; the
/// output is that of the last evaluated step (see [`Pipeline::status`]).
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub(super) source: Arc<dyn DataSource>,
    pub(super) steps: Vec<Step>,
    /// Identity of each step's current content, to tell whether a computed
    /// result still matches the steps it was computed for.
    ids: Vec<u64>,
    next_id: u64,
    /// Computed results of full-pass steps, by step index.
    pub(super) caches: Vec<Option<Arc<Cache>>>,
    /// `levels[0]` is the source; `levels[i + 1]` is the output of
    /// `steps[i]`, for the steps that are evaluated.
    pub(super) levels: Vec<Level>,
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
            Status::Computing { step } => Err(invalid!(StepComputing, step = step + 1)),
            Status::Broken { step, .. } => Err(invalid!(StepBroken, step = step + 1)),
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
        let level = step.with_transform(|t| t.level(self.last()))?;
        if step.needs_full_pass() {
            self.status = Status::Computing {
                step: self.steps.len(),
            };
        } else {
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
            let level = &self.levels[i];
            let result = match &self.caches[i] {
                Some(cache) if step.needs_full_pass() => Ok(level.after_pass(cache)),
                None if step.needs_full_pass() => match step.with_transform(|t| t.level(level)) {
                    Ok(_) => {
                        self.status = Status::Computing { step: i };
                        return;
                    }
                    Err(e) => Err(e),
                },
                _ => step.with_transform(|t| t.level(level)),
            };
            match result {
                Ok(level) => self.levels.push(level),
                Err(e) => {
                    self.status = Status::Broken {
                        step: i,
                        error: Arc::new(e),
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
    pub fn fail(&mut self, key: &[u64], error: Arc<Error>) -> bool {
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
                    self.fail(&key, Arc::new(e.clone()));
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
            return Err(invalid!(StepNotEvaluated, step = steps));
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
    /// the level's order). Only what those columns need is read, down to the
    /// file (#171).
    pub fn read_columns_at(
        &self,
        steps: usize,
        range: Range<usize>,
        names: &[String],
    ) -> Result<RecordBatch> {
        let Some(level) = self.levels.get(steps) else {
            return Err(invalid!(StepNotEvaluated, step = steps));
        };
        let end = range.end.min(level.num_rows);
        let range = range.start.min(end)..end;
        self.read_level_columns(steps, range, names)
    }
}

impl crate::plan::Output for Pipeline {
    fn schema(&self) -> SchemaRef {
        Pipeline::schema(self)
    }

    fn num_rows(&self) -> usize {
        Pipeline::num_rows(self)
    }

    fn read_pieces<T: Send + 'static>(
        &self,
        map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        self.read_parallel(self.evaluated(), 0..self.num_rows(), None, map)
    }
}
