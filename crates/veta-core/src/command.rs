//! Commands: every change to a [`Document`](crate::model::Document) is
//! expressed as one of these and applied by the
//! [`controller`](crate::controller). The GUI and the CLI build the same
//! commands.

use std::ops::Range;

use arrow::datatypes::DataType;

use crate::model::{KeyValue, WriterSettings};
use crate::steps::{Cast, CustomColumn, Fill, Filter, Merge, Replace, Sort, Split, Step, TextOp};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Sets a key/value metadata entry, replacing the value if the key exists.
    SetMetadata {
        key: String,
        value: Option<String>,
    },
    /// Removes a key/value metadata entry.
    RemoveMetadata {
        key: String,
    },
    /// Replaces all key/value metadata at once. Keys must be unique and not
    /// empty.
    ReplaceMetadata(Vec<KeyValue>),
    /// Replaces a column's metadata (stored in the schema). Keys must be
    /// unique and not empty.
    ReplaceColumnMetadata {
        column: String,
        entries: Vec<KeyValue>,
    },
    /// Sets a cell from text, parsed as the column's type. `None`, or blank
    /// text in a non-text column, sets the cell to null.
    SetCell {
        row: usize,
        column: String,
        value: Option<String>,
    },
    /// Inserts `count` empty rows before row `at` (`at` = row count appends).
    InsertRows {
        at: usize,
        count: usize,
    },
    /// Deletes rows. Ranges may overlap and come in any order.
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
    /// Keeps only these columns.
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
    /// Replaces the settings used when saving.
    SetWriterSettings(WriterSettings),
    /// Keeps the rows that match the filter.
    Filter(Filter),
    /// Sorts rows by one or more columns (stable; nulls last).
    Sort(Sort),
    /// Changes a column's type.
    Cast(Cast),
    Replace(Replace),
    /// Trims or changes the case of text columns.
    FormatText {
        columns: Vec<String>,
        op: TextOp,
    },
    /// Fills nulls from the row above (or below).
    Fill(Fill),
    Split(Split),
    Merge(Merge),
    /// Fields of a struct column become columns.
    ExpandStruct {
        column: String,
    },
    /// One row per item of a list or map column.
    ExplodeList {
        column: String,
    },
    /// Adds a column computed from an expression.
    CustomColumn(CustomColumn),
    /// Appends steps (e.g. from a recipe) as one change. Steps that don't
    /// apply are kept and shown as broken.
    AppendSteps {
        steps: Vec<Step>,
    },
    /// Removes the step at `index`. Later steps may become broken.
    RemoveStep {
        index: usize,
    },
    /// Moves the step at `from` to position `to`.
    MoveStep {
        from: usize,
        to: usize,
    },
    /// Replaces the step at `index` (e.g. after editing its settings).
    ReplaceStep {
        index: usize,
        step: Step,
    },
}
