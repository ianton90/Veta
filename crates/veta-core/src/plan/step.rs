//! The transformation trait: what every kind of step answers, and how its
//! output relates to its input.

use std::fmt;
use std::sync::Arc;

use arrow::datatypes::SchemaRef;

use super::cells::CellView;
use super::progress::Progress;
use super::rows::Selection;
use super::store::Table;
use crate::error::Result;
use crate::model::{FileMetadata, WriterSettings};
use crate::source::DataSource;

/// The columns and rows of a step's output.
#[derive(Debug, Clone)]
pub(crate) struct Shape {
    pub(crate) schema: SchemaRef,
    pub(crate) rows: usize,
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
    /// The input, with these cells changed.
    Cells(CellView),
    /// The input, with the columns of this table (same rows, same names)
    /// in place of its own.
    Patch(Arc<Table>),
}

/// What steps are planned with.
#[derive(Debug, Clone, Default)]
pub(crate) struct Context {
    /// Leave data to be read when needed.
    pub(crate) lazy: bool,
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
    /// Checks the step against its input (columns, types, rows) and says
    /// what comes out. Data isn't read, except by sources, which read a
    /// file's footer, or the whole file unless the context is lazy.
    fn plan(&self, input: &Shape, context: &Context) -> Result<Planned>;
}
