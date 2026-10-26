//! Splitting a column's text into columns, and merging columns' text.

use std::sync::Arc;

use arrow::datatypes::{Field, Schema};
use arrow::record_batch::RecordBatch;

use super::derive::ColumnOp;
use crate::error::Result;
use crate::transforms::split::{Merge, Split};

impl ColumnOp for Split {
    fn reads(&self) -> Vec<String> {
        vec![self.column.clone()]
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        self.output_fields(schema)
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        Split::apply(self, batch, schema)
    }
}

impl ColumnOp for Merge {
    fn reads(&self) -> Vec<String> {
        self.columns.clone()
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        self.output_fields(schema)
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        Merge::apply(self, batch, schema)
    }
}
