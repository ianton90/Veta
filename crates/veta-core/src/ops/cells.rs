//! Cell edits, and the snapshot of the edited columns they are grouped into
//! when they take too much memory.

use std::sync::Arc;

use crate::error::{Result, invalid};
use crate::plan::store::Table;
use crate::plan::{CellView, Context, Effect, Node, Planned, Step};
use crate::source::DataSource;

/// Sets cells to new values.
#[derive(Debug)]
pub(crate) struct SetCells(pub(crate) CellView);

impl Step for SetCells {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        for (column, row) in self.0.cells() {
            input
                .schema()
                .index_of(&column)
                .map_err(|_| invalid!(ColumnNotFound, column = column))?;
            if row >= input.rows()? {
                return Err(invalid!(RowPastEnd, row = row, rows = input.rows()?));
            }
        }
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Cells(self.0.clone()),
            file: None,
        })
    }
}

/// Puts the columns of a table (the edited columns, whole) in place of the
/// input's.
#[derive(Debug)]
pub(crate) struct Snapshot(pub(crate) Arc<Table>);

impl Step for Snapshot {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        if self.0.num_rows() != input.rows()? {
            return Err(invalid!(
                RowPastEnd,
                row = self.0.num_rows(),
                rows = input.rows()?
            ));
        }
        for field in self.0.schema().fields() {
            let at = input
                .schema()
                .index_of(field.name())
                .map_err(|_| invalid!(ColumnNotFound, column = field.name()))?;
            if input.schema().field(at).data_type() != field.data_type() {
                return Err(invalid!(EditType, column = field.name()));
            }
        }
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Patch(self.0.clone()),
            file: None,
        })
    }
}
