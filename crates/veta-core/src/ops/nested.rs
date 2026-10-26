//! Nested values: struct fields become columns, fields are added or
//! removed, list items become rows.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use super::derive::ColumnOp;
use crate::display::PathPart;
use crate::error::Result;
use crate::plan::store::Table;
use crate::plan::{Context, Effect, Node, Planned, Step, Task};
use crate::transforms::nested::{
    change_field, change_field_fields, expand_all, expand_levels, explode, explode_fields,
};

/// The fields of struct `column` become columns `<column>.<field>` (and,
/// with `all_levels`, those of structs inside it).
#[derive(Debug)]
pub(crate) struct Unnest {
    pub(crate) column: String,
    pub(crate) all_levels: bool,
}

impl ColumnOp for Unnest {
    fn reads(&self) -> Vec<String> {
        vec![self.column.clone()]
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        let (_, schema) = expand_all(schema, &self.column, self.all_levels)?;
        Ok(schema.fields().iter().map(|f| f.as_ref().clone()).collect())
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        expand_levels(batch, &self.column, self.all_levels, schema)
    }
}

/// Adds a null field `name` of type `add` to the struct at `path` in
/// `column` (`None`: removes it).
#[derive(Debug)]
pub(crate) struct ChangeField {
    pub(crate) column: String,
    pub(crate) path: Vec<PathPart>,
    pub(crate) name: String,
    pub(crate) add: Option<DataType>,
}

impl ColumnOp for ChangeField {
    fn reads(&self) -> Vec<String> {
        vec![self.column.clone()]
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        change_field_fields(
            schema,
            &self.column,
            &self.path,
            &self.name,
            self.add.as_ref(),
        )
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        change_field(batch, &self.column, schema)
    }
}

/// One row per item of list (or map) `column`.
#[derive(Debug)]
pub(crate) struct Explode {
    pub(crate) column: String,
}

impl Step for Explode {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        let schema = input.schema();
        let fields = explode_fields(schema, &self.column)?;
        Ok(Planned {
            schema: Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
            effect: Effect::Later,
            file: None,
        })
    }

    /// The whole output, window by window, kept as a table.
    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let schema = input.schema();
        let output = Arc::new(Schema::new_with_metadata(
            explode_fields(schema, &self.column)?,
            schema.metadata().clone(),
        ));
        let progress = context.progress.as_ref().map(|p| (p, Task::Computing));
        let (column, again) = (self.column.clone(), output.clone());
        let batches = input
            .read_parallel(None, progress, move |batches, _| {
                batches
                    .iter()
                    .map(|b| explode(b, &column, &again))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(Effect::Data(Table::new(output, batches)?))
    }
}
