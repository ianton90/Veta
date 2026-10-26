//! The transformation trait: what every kind of step answers, and how its
//! output relates to its input.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use arrow::array::{ArrayRef, UInt32Array};

use arrow::datatypes::SchemaRef;

use super::cells::CellView;
use super::node::Node;
use super::progress::Progress;
use super::rows::Selection;
use super::store::Table;
use crate::error::{Error, Result, invalid};
use crate::model::{FileMetadata, WriterSettings};
use crate::source::DataSource;

/// Where a column of a deriving step's output comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Col {
    /// The input's column at this position.
    Input(usize),
    /// The step's `n`-th new column.
    New(usize),
}

/// How a step's output is read.
#[derive(Debug, Clone)]
pub(crate) enum Effect {
    /// From this data, leaving the input behind (sources).
    Data(Arc<dyn DataSource>),
    /// Output column `i` is input column `map[i]`: picked, reordered or
    /// renamed (the names come from the output schema).
    Columns(Vec<usize>),
    /// Output rows are these input rows.
    Rows(Arc<Selection>),
    /// Output row `i` is input row `order[i]`.
    Order(Arc<UInt32Array>),
    /// The input with `count` empty rows inserted at row `at`.
    Insert { at: usize, count: usize },
    /// Columns from the input or made by the step ([`Step::derive`]), row
    /// for row: kept when the step runs, derived as read while lazy.
    Derived(Vec<Col>),
    /// Computed by [`Step::compute`] when the step runs (now, or when data
    /// is needed while lazy).
    Later,
    /// The input, with these cells changed.
    Cells(CellView),
    /// The input, with the columns of this table (same rows, same names)
    /// in place of its own.
    Patch(Arc<Table>),
    /// The step doesn't fit its input: reading gives this error.
    Broken(Error),
    /// After a broken step: not planned, reading gives the broken step's
    /// error.
    Unplanned,
}

/// What steps are planned with.
#[derive(Debug, Clone, Default)]
pub(crate) struct Context {
    /// Leave data to be read when needed.
    pub(crate) lazy: bool,
    /// Row steps store their whole output instead of the rows they keep
    /// (eager only).
    pub(crate) in_place: bool,
    pub(crate) progress: Option<Progress>,
}

/// What a step makes of its input.
#[derive(Debug)]
pub(crate) struct Planned {
    pub(crate) schema: SchemaRef,
    pub(crate) effect: Effect,
    /// A file's metadata and writer settings, when the step reads one.
    pub(crate) file: Option<FileSettings>,
}

/// What a file says about how it was written, kept to write it back alike.
#[derive(Debug, Clone, Default)]
pub(crate) struct FileSettings {
    pub(crate) metadata: FileMetadata,
    pub(crate) writer: WriterSettings,
}

/// One kind of step. A new transformation is a new type implementing this
/// trait, plus its [`DataFrame`](crate::frame::DataFrame) method.
pub(crate) trait Step: fmt::Debug + Send + Sync {
    /// Checks the step against its input (columns, types) and says what
    /// comes out. Data isn't read, except by sources, which read a file's
    /// footer, or the whole file unless the context is lazy.
    fn plan(&self, input: &Arc<Node>, context: &Context) -> Result<Planned>;

    /// For [`Effect::Derived`]: the new columns for input rows `rows`.
    fn derive(&self, _input: &Node, _rows: Range<usize>) -> Result<Vec<ArrayRef>> {
        Err(invalid!(ComputeFirst))
    }

    /// For [`Effect::Later`]: how the output is read, once computed.
    fn compute(&self, _input: &Arc<Node>, _context: &Context) -> Result<Effect> {
        Err(invalid!(ComputeFirst))
    }
}
