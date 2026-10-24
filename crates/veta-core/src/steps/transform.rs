//! What each kind of step answers for itself (#186): the shape of its
//! output, how to read a window of it, and its full pass if it needs one.
//! The pipeline only delegates, through [`Step::with_transform`]. Adding a
//! kind of step means its transform (in `transforms/`, or here in `steps/`
//! for the row steps), its recipe form, its dialog and its names.

use std::ops::Range;
use std::sync::Arc;

use arrow::datatypes::{Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;

use super::Step;
use super::compute::{Cache, ComputeJob};
use super::level::Level;
use super::pipeline::Pipeline;
use super::read::{project_schema, with_schema};
use super::rows::{DeleteRows, InsertRows};
use crate::error::{Result, invalid};
use crate::transforms::columns::{
    AddColumn, KeepColumns, MoveColumn, RemoveColumns, RemoveEmptyColumn, RenameColumn,
};
use crate::transforms::nested::steps::{
    AddField, CollectList, CombineStruct, ExpandStruct, ExplodeList, RemoveField,
};
use crate::transforms::text::FormatText;

/// One kind of step.
pub(crate) trait Transform {
    /// The shape of the data after the step, checking the step against
    /// `input`. A step that needs a full pass only checks here that it can
    /// run; its shape comes from the pass ([`Level::after_pass`]).
    fn level(&self, input: &Level) -> Result<Level>;

    /// Whether evaluating the step needs a full pass over its input (done
    /// in the background, then cached).
    fn full_pass(&self) -> bool {
        false
    }

    /// Computes the step on `batch`, a window of its input whose first row
    /// is `first_row`; `output` is the step's output schema. Steps that
    /// read their input otherwise (rows, column picks, full passes) say so
    /// in [`Transform::read`] instead.
    fn apply(
        &self,
        batch: &RecordBatch,
        first_row: usize,
        output: &SchemaRef,
    ) -> Result<RecordBatch> {
        let _ = (batch, first_row, output);
        Err(invalid!(ComputeFirst))
    }

    /// Input columns the step changes in place. With the columns it adds,
    /// they are the ones it computes; the others pass through as they are.
    fn changed_columns(&self) -> Vec<&str> {
        Vec::new()
    }

    /// Whether the step refers to rows by position, so changing an earlier
    /// step can make it apply to different rows.
    fn positional(&self) -> bool {
        false
    }

    /// When output column `column` holds its input's values as they are
    /// (only renamed, moved, reordered or with null rows added): the input
    /// column's name and how many null rows the step adds. `None` when the
    /// step may change them, or doesn't say.
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        let _ = column;
        None
    }

    /// Rows `range` of the step's output: every column (`names` `None`) or
    /// only the named ones, in the output's order. By default the window is
    /// [`applied`](Transform::apply); with names, columns the step doesn't
    /// compute are read straight from its input.
    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        let Some(names) = names else {
            return self.apply(
                &input.read(range.clone(), None)?,
                range.start,
                input.output(),
            );
        };
        let (before, after) = (input.schema(), input.output());
        let changed = self.changed_columns();
        let computed =
            |name: &String| before.index_of(name).is_err() || changed.contains(&name.as_str());
        let wanted = project_schema(after, names);
        if wanted.fields().iter().any(|f| computed(f.name())) {
            return crate::source::project(&self.read(input, range, None)?, names);
        }
        input.pick(&input.read(range, Some(names))?, Some(names), |name| {
            name.to_owned()
        })
    }

    /// Runs the step's full pass over its `input_rows` input rows.
    /// `progress` gets the fraction done (0–1) and returns `false` to
    /// cancel.
    fn compute(
        &self,
        job: &ComputeJob,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Cache> {
        let _ = (job, input_rows, progress);
        Err(invalid!(NoComputeNeeded))
    }
}

/// For steps read only through their full pass's result.
pub(crate) fn compute_first() -> Result<RecordBatch> {
    Err(invalid!(ComputeFirst))
}

/// A step's input, to read it from: the level before the step.
pub(crate) struct Input<'a> {
    pub(super) pipeline: &'a Pipeline,
    /// The input's level (the step's index).
    pub(super) level: usize,
}

impl Input<'_> {
    /// The input's schema.
    pub(crate) fn schema(&self) -> &SchemaRef {
        &self.pipeline.levels[self.level].schema
    }

    /// The step's output schema.
    pub(crate) fn output(&self) -> &SchemaRef {
        &self.pipeline.levels[self.level + 1].schema
    }

    /// Input rows `range`: every column, or only those named.
    pub(crate) fn read(
        &self,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        match names {
            None => self.pipeline.read_level(self.level, range),
            Some(names) => self.pipeline.read_level_columns(self.level, range, names),
        }
    }

    /// The output columns `names` (all when `None`), each taken from
    /// `batch` (read from the input) under `source(name)`.
    pub(crate) fn pick(
        &self,
        batch: &RecordBatch,
        names: Option<&[String]>,
        source: impl Fn(&str) -> String,
    ) -> Result<RecordBatch> {
        let schema = self.output_schema(names);
        let input = batch.schema();
        let columns = schema
            .fields()
            .iter()
            .map(|f| Ok(batch.column(input.index_of(&source(f.name()))?).clone()))
            .collect::<Result<Vec<_>>>()?;
        with_schema(schema, columns, batch.num_rows())
    }

    /// The output's schema with only `names`, or all of it.
    pub(crate) fn output_schema(&self, names: Option<&[String]>) -> SchemaRef {
        match names {
            Some(names) => project_schema(self.output(), names),
            None => self.output().clone(),
        }
    }
}

impl Level {
    /// The level with these fields (keeping the schema's metadata) and
    /// `num_rows` rows.
    pub(crate) fn with_fields(&self, fields: Vec<Field>, num_rows: usize) -> Level {
        Level {
            schema: Arc::new(Schema::new_with_metadata(
                fields,
                self.schema.metadata().clone(),
            )),
            num_rows,
        }
    }

    /// The fields, to change into the next level's.
    pub(crate) fn fields(&self) -> Vec<Field> {
        self.schema
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect()
    }
}

impl Step {
    /// Runs `f` with this step's transform: the one place that knows every
    /// kind of step.
    pub(crate) fn with_transform<R>(&self, f: impl FnOnce(&dyn Transform) -> R) -> R {
        match self {
            Step::EditCells(edits) => f(edits),
            Step::InsertRows { at, count } => f(&InsertRows {
                at: *at,
                count: *count,
            }),
            Step::DeleteRows { rows } => f(&DeleteRows { rows }),
            Step::AddColumn {
                name,
                data_type,
                at,
            } => f(&AddColumn {
                name,
                data_type,
                at: *at,
            }),
            Step::RemoveColumns { names } => f(&RemoveColumns { names }),
            Step::KeepColumns { names } => f(&KeepColumns { names }),
            Step::RenameColumn { from, to } => f(&RenameColumn { from, to }),
            Step::MoveColumn { name, to } => f(&MoveColumn { name, to: *to }),
            Step::Filter(filter) => f(filter),
            Step::Sort(sort) => f(sort),
            Step::Cast(cast) => f(cast),
            Step::Replace(replace) => f(replace),
            Step::FormatText { columns, op } => f(&FormatText { columns, op: *op }),
            Step::Fill(fill) => f(fill),
            Step::Split(split) => f(split),
            Step::Merge(merge) => f(merge),
            Step::CustomColumn(custom) => f(custom),
            Step::ExpandStruct { column, all_levels } => f(&ExpandStruct {
                column,
                all_levels: *all_levels,
            }),
            Step::ExplodeList { column } => f(&ExplodeList { column }),
            Step::AddField {
                column,
                path,
                name,
                data_type,
            } => f(&AddField {
                column,
                path,
                name,
                data_type,
            }),
            Step::RemoveField { column, path, name } => f(&RemoveField { column, path, name }),
            Step::CollectList { keys, columns } => f(&CollectList { keys, columns }),
            Step::CombineStruct { columns, name } => f(&CombineStruct { columns, name }),
            Step::RemoveEmptyColumn { name } => f(&RemoveEmptyColumn { name }),
        }
    }
}
