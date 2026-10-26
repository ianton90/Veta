//! The plan behind a [`DataFrame`](crate::frame::DataFrame): its steps as a
//! chain of nodes, what each step keeps, and the stored tables.

mod cells;
mod history;
mod node;
mod progress;
mod rows;
mod step;
pub(crate) mod store;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;

pub(crate) use cells::CellView;
pub(crate) use history::History;
pub(crate) use node::Node;
pub use progress::{Progress, Task};
pub use rows::Rows;
pub(crate) use rows::Selection;
pub(crate) use step::{Col, Context, Effect, FileSettings, Planned, Step};

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
        Node::schema(self).clone()
    }

    fn num_rows(&self) -> usize {
        self.rows().unwrap_or(0)
    }

    fn read_pieces<T: Send + 'static>(
        &self,
        map: impl Fn(Vec<RecordBatch>, usize) -> crate::error::Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        self.read_parallel(None, None, map)
    }
}
