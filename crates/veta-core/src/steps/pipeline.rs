use std::ops::Range;
use std::sync::Arc;

use arrow::array::{ArrayRef, new_null_array};
use arrow::compute::concat_batches;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::edits::CellEdits;
use super::level::Level;
use super::rows::kept_ranges;
use super::{Step, invalid};
use crate::error::Result;
use crate::source::DataSource;

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
        let level = self.last().next(&step)?;
        let id = self.new_id();
        self.steps.push(step);
        self.ids.push(id);
        self.levels.push(level);
        Ok(())
    }

    /// Removes and returns the last step.
    pub fn pop(&mut self) -> Option<Step> {
        let step = self.steps.pop()?;
        self.ids.pop();
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
            match self.levels[i].next(step) {
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

    /// Reads rows of level `level`; `range` must be within it.
    fn read_level(&self, level: usize, range: Range<usize>) -> Result<RecordBatch> {
        if level == 0 {
            return self.source.read(range);
        }
        let schema = self.levels[level].schema.clone();
        let previous = level - 1;
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
            Step::RemoveColumns { .. } | Step::RenameColumn { .. } | Step::MoveColumn { .. } => {
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
