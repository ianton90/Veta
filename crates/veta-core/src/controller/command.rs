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
    /// Sets a field of a struct cell from text, parsed as the field's type.
    /// `path` names the fields from the column down (through structs only).
    /// `None`, or blank text in a non-text field, sets null.
    SetField {
        row: usize,
        column: String,
        path: Vec<String>,
        value: Option<String>,
    },
    /// Sets item `index` of a list in a cell from text, parsed as the item
    /// type. `list` names the struct fields from the column down to the list
    /// (empty: the cell is the list). `index` may be the list's length,
    /// which appends. A null list counts as empty.
    SetListItem {
        row: usize,
        column: String,
        list: Vec<String>,
        index: usize,
        value: Option<String>,
    },
    /// Inserts a null item at `index` (up to the list's length) in a list in
    /// a cell; `list` as for [`Command::SetListItem`].
    InsertListItem {
        row: usize,
        column: String,
        list: Vec<String>,
        index: usize,
    },
    /// Removes item `index` of a list in a cell; `list` as for
    /// [`Command::SetListItem`].
    RemoveListItem {
        row: usize,
        column: String,
        list: Vec<String>,
        index: usize,
    },
    /// Sets the value of entry `index` of a map in a cell from text, parsed
    /// as the value type. `map` names the struct fields from the column down
    /// to the map (empty: the cell is the map). Only maps with text keys can
    /// be edited by entry.
    SetMapValue {
        row: usize,
        column: String,
        map: Vec<String>,
        index: usize,
        value: Option<String>,
    },
    /// Renames the key of entry `index` of a map in a cell; keys must stay
    /// unique. `map` as for [`Command::SetMapValue`].
    SetMapKey {
        row: usize,
        column: String,
        map: Vec<String>,
        index: usize,
        key: String,
    },
    /// Inserts an entry with a new key and a null value at `index` (up to the
    /// number of entries) in a map in a cell. A null map counts as empty.
    InsertMapEntry {
        row: usize,
        column: String,
        map: Vec<String>,
        index: usize,
        key: String,
    },
    /// Removes entry `index` of a map in a cell.
    RemoveMapEntry {
        row: usize,
        column: String,
        map: Vec<String>,
        index: usize,
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
    /// Fields of a struct column become columns; with `all_levels`, struct
    /// fields inside it too.
    ExpandStruct {
        column: String,
        all_levels: bool,
    },
    /// One row per item of a list or map column.
    ExplodeList {
        column: String,
    },
    /// Adds a null field at the end of a struct column.
    AddField {
        column: String,
        name: String,
        data_type: DataType,
    },
    /// Removes a field from a struct column.
    RemoveField {
        column: String,
        name: String,
    },
    /// One row per group of equal keys, other chosen columns as lists.
    CollectList {
        keys: Vec<String>,
        columns: Vec<String>,
    },
    /// Columns become the fields of one struct column.
    CombineStruct {
        columns: Vec<String>,
        name: String,
    },
    /// Adds a column computed from an expression.
    CustomColumn(CustomColumn),
    /// Appends steps (e.g. from a recipe) as one change. Steps that don't
    /// apply are kept and shown as broken.
    AppendSteps {
        steps: Vec<Step>,
    },
    /// Replaces all steps (e.g. with a recipe's) as one change.
    ReplaceSteps {
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
