//! Applied steps and the engine that evaluates them.
//!
//! A document's data is its source followed by a list of [`Step`]s, each
//! transforming the output of the previous one (Power Query's "Applied
//! Steps"). [`Pipeline`] evaluates the steps lazily over a row window, so only
//! the rows on screen (or the batch being saved) are ever computed.
//!
//! Steps refer to columns by name and to rows by their index in the previous
//! step's output.

pub mod checkpoint;
mod compute;
mod edits;
mod level;
mod pipeline;
mod read;
mod rows;
mod transform;

use std::ops::Range;

use arrow::datatypes::DataType;

use crate::display::PathPart;

pub use crate::formulas::{CustomColumn, function_signatures};
pub use crate::transforms::cast::{Cast, CastOptions, CastPreview, OnError};
pub use crate::transforms::fill::Fill;
pub use crate::transforms::filter::{Condition, Filter, FilterOp};
pub use crate::transforms::sort::{Sort, SortKey};
pub use crate::transforms::split::{Merge, Split, SplitBy, count_parts};
pub use crate::transforms::text::{Replace, TextOp};
pub(crate) use compute::{Cache, PASS_ROWS, RowMap};
pub use compute::{ComputeJob, Computed};
pub use edits::CellEdits;
pub(crate) use level::Level;
pub use pipeline::{Pipeline, Status};
pub(crate) use read::with_schema;
pub use rows::normalize_ranges;
pub(crate) use transform::{Input, Transform, compute_first};

/// Most rows one step inserts (a recipe could ask for any number).
pub const MAX_INSERT_ROWS: usize = 10_000_000;

/// One transformation of the data.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Replaces individual cell values.
    EditCells(CellEdits),
    /// Inserts `count` empty rows before row `at`.
    InsertRows {
        at: usize,
        count: usize,
    },
    /// Deletes rows. Ranges are sorted, disjoint and non-empty.
    DeleteRows {
        rows: Vec<Range<usize>>,
    },
    /// Adds an empty column at position `at`.
    AddColumn {
        name: String,
        data_type: DataType,
        at: usize,
    },
    RemoveColumns {
        names: Vec<String>,
    },
    /// Keeps only these columns, in their current order.
    KeepColumns {
        names: Vec<String>,
    },
    RenameColumn {
        from: String,
        to: String,
    },
    /// Moves a column to position `to`.
    MoveColumn {
        name: String,
        to: usize,
    },
    /// Keeps the rows that match.
    Filter(Filter),
    Sort(Sort),
    /// Changes a column's type.
    Cast(Cast),
    Replace(Replace),
    /// Trims or changes the case of text columns.
    FormatText {
        columns: Vec<String>,
        op: TextOp,
    },
    /// Fills nulls from neighbouring rows.
    Fill(Fill),
    Split(Split),
    Merge(Merge),
    CustomColumn(CustomColumn),
    /// Fields of a struct column become columns `<column>.<field>`; with
    /// `all_levels`, struct fields are expanded too (`a.b.c`).
    ExpandStruct {
        column: String,
        all_levels: bool,
    },
    /// One row per item of a list (or entry of a map) column.
    ExplodeList {
        column: String,
    },
    /// Adds a null field `name` at the end of the struct at `path` in
    /// column `column` (empty: the column is the struct). `path` steps
    /// through struct fields, list items and map values (their positions
    /// don't matter: every item or value gets the field).
    AddField {
        column: String,
        path: Vec<PathPart>,
        name: String,
        data_type: DataType,
    },
    /// Removes field `name` from the struct at `path` in column `column`.
    RemoveField {
        column: String,
        path: Vec<PathPart>,
        name: String,
    },
    /// One row per group of equal `keys` (ordered by them), with `columns`
    /// collected into lists; other columns are dropped. Reverses
    /// [`Step::ExplodeList`].
    CollectList {
        keys: Vec<String>,
        columns: Vec<String>,
    },
    /// Columns become the fields of one struct column `name`, placed where
    /// the first of them was. A `<name>.` prefix is dropped from field
    /// names, so it reverses [`Step::ExpandStruct`].
    CombineStruct {
        columns: Vec<String>,
        name: String,
    },
    /// Removes a column if every value in it is null; fails otherwise
    /// (e.g. a cast's error column when nothing failed).
    RemoveEmptyColumn {
        name: String,
    },
}

impl Step {
    /// Whether evaluating the step needs a full pass over its input (done in
    /// the background, then cached).
    pub fn needs_full_pass(&self) -> bool {
        self.with_transform(|t| t.full_pass())
    }

    /// Whether the step refers to rows by position, so changing an earlier
    /// step can make it apply to different rows.
    pub fn is_positional(&self) -> bool {
        self.with_transform(|t| t.positional())
    }
}

/// The schema after `step`, given its input schema (without reading data).
pub fn schema_after(
    schema: &arrow::datatypes::SchemaRef,
    step: &Step,
) -> crate::error::Result<arrow::datatypes::SchemaRef> {
    // Row counts are unknown here; a large one lets row checks pass.
    let level = level::Level {
        schema: schema.clone(),
        num_rows: usize::MAX / 4,
    };
    let after = step.with_transform(|t| t.level(&level))?;
    match step.needs_full_pass() {
        true => Ok(schema.clone()),
        false => Ok(after.schema),
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Row ranges, not a range of values.
mod tests;

/// A path to a type inside a column as text: `.field`, `[]` for list items,
/// `{}` for map values.
pub fn type_path(path: &[PathPart]) -> String {
    path.iter()
        .map(|part| match part {
            PathPart::Field(name) => format!(".{name}"),
            PathPart::Index(_) => "[]".into(),
            PathPart::Entry(_) => "{}".into(),
        })
        .collect()
}
