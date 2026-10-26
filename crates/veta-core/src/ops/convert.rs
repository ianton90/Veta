//! Column operations that change values in place: cast, replace, format
//! text, fill nulls with a value.

use std::sync::Arc;

use arrow::array::{ArrayRef, Scalar};
use arrow::compute::{is_null, kernels::zip::zip};
use arrow::datatypes::{Field, Schema};
use arrow::record_batch::RecordBatch;

use super::derive::ColumnOp;
use crate::error::{Result, invalid};
use crate::transforms::cast::Cast;
use crate::transforms::text::{Replace, TextOp, map_columns};

impl ColumnOp for Cast {
    fn reads(&self) -> Vec<String> {
        vec![self.column.clone()]
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        self.output_fields(schema)
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        Cast::apply(self, batch, schema)
    }
}

impl ColumnOp for Replace {
    fn reads(&self) -> Vec<String> {
        vec![self.column.clone()]
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        self.validate(schema)?;
        Ok(schema.fields().iter().map(|f| f.as_ref().clone()).collect())
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        map_columns(batch, schema, std::slice::from_ref(&self.column), |c| {
            Replace::apply(self, c)
        })
    }
}

/// Formats the text of `columns`.
#[derive(Debug)]
pub(crate) struct FormatText {
    pub(crate) columns: Vec<String>,
    pub(crate) op: TextOp,
}

impl ColumnOp for FormatText {
    fn reads(&self) -> Vec<String> {
        self.columns.clone()
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        self.op.validate(schema, &self.columns)?;
        Ok(schema.fields().iter().map(|f| f.as_ref().clone()).collect())
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        map_columns(batch, schema, &self.columns, |c| self.op.apply(c))
    }
}

/// Fills the nulls of `column` with `value` (one value of its type).
#[derive(Debug)]
pub(crate) struct FillValue {
    pub(crate) column: String,
    pub(crate) value: ArrayRef,
}

impl ColumnOp for FillValue {
    fn reads(&self) -> Vec<String> {
        vec![self.column.clone()]
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        let field = schema
            .field_with_name(&self.column)
            .map_err(|_| invalid!(ColumnNotFound, column = self.column))?;
        if field.data_type() != self.value.data_type() {
            return Err(invalid!(EditType, column = self.column));
        }
        Ok(schema.fields().iter().map(|f| f.as_ref().clone()).collect())
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        map_columns(
            batch,
            schema,
            std::slice::from_ref(&self.column),
            |column| {
                Ok(zip(
                    &is_null(column)?,
                    &Scalar::new(self.value.clone()),
                    column,
                )?)
            },
        )
    }
}
