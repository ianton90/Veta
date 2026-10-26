//! Column operations: steps that make new columns from some input columns,
//! row for row. Each answers which columns it reads, what comes out of a
//! schema and of a batch; the rest (keeping the new columns, deriving them
//! as read while lazy, passing the other columns through) is shared.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use arrow::array::ArrayRef;
use arrow::datatypes::{Field, Schema};
use arrow::record_batch::RecordBatch;

use crate::error::{Result, invalid};
use crate::plan::{Col, Context, Effect, Node, Planned, Step};

/// A step that makes columns from columns, row for row.
pub(crate) trait ColumnOp: fmt::Debug + Send + Sync {
    /// The input columns it reads.
    fn reads(&self) -> Vec<String>;

    /// Checks it against `schema` and gives the output fields. Fields not
    /// read keep their place and name; read ones may change, move or go.
    fn fields(&self, schema: &Schema) -> Result<Vec<Field>>;

    /// The output of `batch`, which holds only the columns read;
    /// `schema` is [`Self::fields`] of the batch's schema.
    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch>;
}

impl<T: ColumnOp> Step for T {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        let schema = input.schema();
        let output = Schema::new_with_metadata(self.fields(schema)?, schema.metadata().clone());
        let reads = self.reads();
        let read = read_schema(schema, &reads)?;
        let made = self.fields(&read)?;
        let map = output
            .fields()
            .iter()
            .map(|field| match schema.index_of(field.name()) {
                Ok(i) if !reads.contains(field.name()) => Ok(Col::Input(i)),
                _ => made
                    .iter()
                    .position(|f| f.name() == field.name())
                    .map(Col::New)
                    .ok_or_else(|| invalid!(ColumnNotFound, column = field.name())),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Planned {
            schema: Arc::new(output),
            effect: Effect::Derived(map),
            file: None,
        })
    }

    fn derive(&self, input: &Node, rows: Range<usize>) -> Result<Vec<ArrayRef>> {
        // In the input's order, as when planned.
        let mut positions = self
            .reads()
            .iter()
            .map(|name| input.schema().index_of(name))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        positions.sort_unstable();
        let batch = input.read(rows, &positions)?;
        let schema = Arc::new(Schema::new_with_metadata(
            self.fields(&batch.schema())?,
            batch.schema().metadata().clone(),
        ));
        Ok(self.apply(&batch, &schema)?.columns().to_vec())
    }
}

/// The columns of `schema` named in `reads`, in their order there.
fn read_schema(schema: &Schema, reads: &[String]) -> Result<Schema> {
    let positions: Vec<usize> = schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| reads.contains(f.name()))
        .map(|(i, _)| i)
        .collect();
    if let Some(missing) = reads.iter().find(|r| schema.index_of(r).is_err()) {
        return Err(invalid!(ColumnNotFound, column = missing));
    }
    Ok(schema.project(&positions)?)
}
