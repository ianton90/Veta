//! [`Column`]: one column of a frame.

use arrow::array::ArrayRef;
use arrow::compute::concat;
use arrow::datatypes::{DataType, Field, Schema};

use super::DataFrame;
use crate::error::{Result, invalid};
use crate::ops::position;

/// A column given by name or position.
pub trait ColumnRef {
    /// Its position in `schema`.
    fn position(&self, schema: &Schema) -> Result<usize>;
}

impl ColumnRef for &str {
    fn position(&self, schema: &Schema) -> Result<usize> {
        position(schema, self)
    }
}

impl ColumnRef for String {
    fn position(&self, schema: &Schema) -> Result<usize> {
        position(schema, self)
    }
}

impl ColumnRef for &String {
    fn position(&self, schema: &Schema) -> Result<usize> {
        position(schema, self)
    }
}

impl ColumnRef for usize {
    fn position(&self, schema: &Schema) -> Result<usize> {
        match *self < schema.fields().len() {
            true => Ok(*self),
            false => Err(invalid!(ColumnPosition, position = self)),
        }
    }
}

/// One column of a frame.
#[derive(Debug, Clone)]
pub struct Column {
    frame: DataFrame,
    position: usize,
}

impl Column {
    pub(super) fn new(frame: DataFrame, position: usize) -> Self {
        Self { frame, position }
    }

    fn field(&self) -> &Field {
        self.frame.schema_ref().field(self.position)
    }

    pub fn name(&self) -> &str {
        self.field().name()
    }

    pub fn data_type(&self) -> &DataType {
        self.field().data_type()
    }

    /// Rows, nulls included.
    pub fn len(&self) -> Result<usize> {
        self.frame.num_rows()
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Every value, as one array.
    pub fn values(&self) -> Result<ArrayRef> {
        let pieces: Vec<ArrayRef> = self
            .frame
            .node
            .read_parallel(Some(vec![self.position]), |batches, _| {
                Ok(batches
                    .into_iter()
                    .map(|b| b.column(0).clone())
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        if pieces.is_empty() {
            return Ok(arrow::array::new_empty_array(self.data_type()));
        }
        let parts: Vec<&dyn arrow::array::Array> = pieces.iter().map(|a| a.as_ref()).collect();
        Ok(concat(&parts)?)
    }
}
