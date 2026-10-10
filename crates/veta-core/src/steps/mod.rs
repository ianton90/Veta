//! Applied steps and the engine that evaluates them.
//!
//! A document's data is its source followed by a list of [`Step`]s, each
//! transforming the output of the previous one (Power Query's "Applied
//! Steps"). [`Pipeline`] evaluates the steps lazily over a row window, so only
//! the rows on screen (or the batch being saved) are ever computed.
//!
//! Steps refer to columns by name and to rows by their index in the previous
//! step's output.

mod compute;
mod edits;
mod filter;
mod level;
mod pipeline;
mod rows;

use std::ops::Range;

use arrow::datatypes::DataType;

use crate::display::type_name;
use crate::error::Error;

pub use compute::{ComputeJob, Computed};
pub use edits::CellEdits;
pub use filter::{Condition, Filter, FilterOp};
pub use pipeline::{Pipeline, Status};
pub use rows::normalize_ranges;

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
}

impl Step {
    /// Whether evaluating the step needs a full pass over its input (done in
    /// the background, then cached).
    pub fn needs_full_pass(&self) -> bool {
        matches!(self, Step::Filter(_))
    }

    /// Whether the step refers to rows by position, so changing an earlier
    /// step can make it apply to different rows.
    pub fn is_positional(&self) -> bool {
        matches!(
            self,
            Step::EditCells(_) | Step::InsertRows { .. } | Step::DeleteRows { .. }
        )
    }

    /// Short description for the applied steps list.
    pub fn describe(&self) -> String {
        fn plural(n: usize, one: &str) -> String {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {one}s")
            }
        }
        match self {
            Step::EditCells(edits) => format!("Edited {}", plural(edits.len(), "cell")),
            Step::InsertRows { count, .. } => format!("Inserted {}", plural(*count, "row")),
            Step::DeleteRows { rows } => {
                let n = rows.iter().map(|r| r.len()).sum();
                format!("Deleted {}", plural(n, "row"))
            }
            Step::AddColumn {
                name, data_type, ..
            } => {
                format!("Added column {name} ({})", type_name(data_type))
            }
            Step::RemoveColumns { names } => match names.as_slice() {
                [one] => format!("Removed column {one}"),
                _ => format!("Removed {} columns", names.len()),
            },
            Step::RenameColumn { from, to } => format!("Renamed {from} to {to}"),
            Step::MoveColumn { name, .. } => format!("Moved column {name}"),
            Step::Filter(filter) => filter.describe(),
        }
    }
}

fn invalid(message: String) -> Error {
    Error::InvalidCommand(message)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Row ranges, not a range of values.
mod tests;
