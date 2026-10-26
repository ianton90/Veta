//! Filling nulls with the value above (down) or below (up).

use std::sync::Arc;

use arrow::array::ArrayRef;
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;

use crate::error::{Result, invalid};
use crate::plan::store::Table;
use crate::plan::{Context, Effect, Node, Planned, Step, Task};
use crate::transforms::fill::{carry_out, fill};

/// Fills the nulls of `column` with the nearest value above it, or below
/// it when `up`. Each piece depends on the last, so the column is computed
/// whole.
#[derive(Debug)]
pub(crate) struct FillNearest {
    pub(crate) column: String,
    pub(crate) up: bool,
}

impl Step for FillNearest {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        input
            .schema()
            .index_of(&self.column)
            .map_err(|_| invalid!(ColumnNotFound, column = self.column))?;
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Later,
            file: None,
        })
    }

    /// The filled column, in one pass (from the end when filling up), kept
    /// as a table laid over the input.
    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let position = input.schema().index_of(&self.column)?;
        let schema = Arc::new(Schema::new(vec![input.schema().field(position).clone()]));
        let pieces: Vec<ArrayRef> = input
            .read_parallel(
                Some(vec![position]),
                context.progress.as_ref().map(|p| (p, Task::Computing)),
                |batches, _| {
                    Ok(batches
                        .into_iter()
                        .map(|b| b.column(0).clone())
                        .collect::<Vec<_>>())
                },
            )
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        let mut filled = Vec::with_capacity(pieces.len());
        let mut carry = None;
        let order: Box<dyn Iterator<Item = &ArrayRef>> = match self.up {
            true => Box::new(pieces.iter().rev()),
            false => Box::new(pieces.iter()),
        };
        for piece in order {
            filled.push(fill(piece, carry.as_ref(), self.up)?);
            carry = carry_out(piece, carry, self.up)?;
        }
        if self.up {
            filled.reverse();
        }
        let batches = filled
            .into_iter()
            .map(|column| Ok(RecordBatch::try_new(schema.clone(), vec![column])?))
            .collect::<Result<Vec<_>>>()?;
        Ok(Effect::Patch(Table::new(schema, batches)?))
    }
}
