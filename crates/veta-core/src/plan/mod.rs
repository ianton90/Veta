//! The plan behind a [`DataFrame`](crate::frame::DataFrame): its steps as a
//! chain of nodes, what each step keeps, and the stored tables.

mod node;
mod rows;
mod step;
pub(crate) mod store;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

pub(crate) use node::Node;
pub use rows::Rows;
pub(crate) use rows::Selection;
pub(crate) use step::{Effect, FileSettings, Planned, Shape, Step};

/// Data that writers read in pieces on several threads.
pub(crate) trait Output {
    fn schema(&self) -> SchemaRef;

    fn num_rows(&self) -> usize;

    /// Every row, in pieces handed to `map` in order, with the first row of
    /// each.
    fn read_pieces<T: Send + 'static>(
        &self,
        map: impl Fn(Vec<RecordBatch>, usize) -> crate::error::Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T>;
}

impl Output for std::sync::Arc<Node> {
    fn schema(&self) -> SchemaRef {
        self.shape.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.shape.rows
    }

    fn read_pieces<T: Send + 'static>(
        &self,
        map: impl Fn(Vec<RecordBatch>, usize) -> crate::error::Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        self.read_parallel(None, map)
    }
}
