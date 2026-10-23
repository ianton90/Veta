//! Commands: every change to a [`Document`](crate::model::Document) is
//! expressed as one of these and applied by the
//! [`controller`](crate::controller). The GUI and the CLI build the same
//! commands.

use crate::display::PathPart;
use crate::model::{KeyValue, WriterSettings};
use crate::steps::Step;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Sets a key/value metadata entry, replacing the value if the key exists.
    SetMetadata { key: String, value: Option<String> },
    /// Removes a key/value metadata entry.
    RemoveMetadata { key: String },
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
    /// Sets a block of cells from text, starting at `row` and the column at
    /// index `column`, as one change. Rows are added at the end if the
    /// block reaches past the last row. `None` sets null.
    PasteCells {
        row: usize,
        column: usize,
        values: Vec<Vec<Option<String>>>,
    },
    /// Like [`Command::PasteCells`], into columns or fields of struct
    /// columns: value `j` of each line goes to `targets[j]`, a column index
    /// and the field names down to the field (empty for the column).
    PasteFields {
        row: usize,
        targets: Vec<(usize, Vec<String>)>,
        values: Vec<Vec<Option<String>>>,
    },
    /// Sets the value at `path` inside a nested cell from text, parsed as
    /// that value's type. `path` goes down through struct fields, list items
    /// and map entries (by position); missing struct fields, and structs
    /// under a null one, are created. A list index equal to the list's length
    /// appends. `None`, or blank text in a non-text value, sets null. The
    /// path can't be empty (that is [`Command::SetCell`]).
    SetValue {
        row: usize,
        column: String,
        path: Vec<PathPart>,
        value: Option<String>,
    },
    /// Inserts an item at `index` (up to the number of items) in the list or
    /// map at `path` in a cell (empty: the cell is it). A list gets a null
    /// item and takes no key; a map gets an entry with `key` (read as the key
    /// type; not empty, not already used) and a null value. A null list or
    /// map counts as empty.
    InsertItem {
        row: usize,
        column: String,
        path: Vec<PathPart>,
        index: usize,
        key: Option<String>,
    },
    /// Removes item `index` of the list, or entry `index` of the map, at
    /// `path` in a cell.
    RemoveItem {
        row: usize,
        column: String,
        path: Vec<PathPart>,
        index: usize,
    },
    /// Renames the key of entry `index` of the map at `path` in a cell (read
    /// as the key type; not empty, not already used).
    SetKey {
        row: usize,
        column: String,
        path: Vec<PathPart>,
        index: usize,
        key: String,
    },
    /// Adds a step at the end. Row ranges to delete may overlap and come in
    /// any order; renaming a column keeps its writer settings and metadata;
    /// a cast with an error column is followed by a step removing that
    /// column if nothing failed (see [`settle`](crate::controller::settle)).
    AddStep(Step),
    /// Replaces the settings used when saving.
    SetWriterSettings(WriterSettings),
    /// Appends steps (e.g. from a recipe) as one change. Steps that don't
    /// apply are kept and shown as broken.
    AppendSteps { steps: Vec<Step> },
    /// Replaces all steps (e.g. with a recipe's) as one change.
    ReplaceSteps { steps: Vec<Step> },
    /// Removes the step at `index`. Later steps may become broken.
    RemoveStep { index: usize },
    /// Moves the step at `from` to position `to`.
    MoveStep { from: usize, to: usize },
    /// Replaces the step at `index` (e.g. after editing its settings).
    ReplaceStep { index: usize, step: Step },
}
