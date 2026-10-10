//! Commands: every change to a [`Document`](crate::model::Document) is
//! expressed as one of these and applied by the
//! [`controller`](crate::controller). The GUI and the CLI build the same
//! commands.

use std::ops::Range;

use arrow::datatypes::DataType;

use crate::model::{KeyValue, WriterSettings};
use crate::steps::{Filter, Step};

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
