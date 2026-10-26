//! Transformations: one type per kind of step, each implementing
//! [`Step`](crate::plan::Step).

mod cells;
mod columns;
mod rows;

pub(crate) use cells::{SetCells, Snapshot};
pub use columns::Columns;
pub(crate) use columns::{KeepColumns, position};
pub(crate) use rows::KeepRows;
