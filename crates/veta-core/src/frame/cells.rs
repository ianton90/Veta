//! Setting single cells.

use std::sync::Arc;

use arrow::compute::{CastOptions, cast_with_options};

use super::{ColumnRef, DataFrame, IntoValue};
use crate::error::{Result, invalid};
use crate::ops::{SetCells, Snapshot};
use crate::plan::store::Table;
use arrow::record_batch::RecordBatch;

use crate::plan::{CellView, Effect, Node, Progress, Task};

impl DataFrame {
    /// This frame with `row` of `column` set to `value`, converted to the
    /// column's type: `df.set(3, "amount", 120)?`, `df.set(3, "note", Null)?`.
    ///
    /// Edits in a row are one step; each can be undone on its own until
    /// they take more than [`crate::memory::cell_edits_limit`], when they
    /// are grouped into a snapshot of the edited columns, undone as one.
    pub fn set(&self, row: usize, column: impl ColumnRef, value: impl IntoValue) -> Result<Self> {
        self.check_not_broken()?;
        let rows = self.num_rows()?;
        if row >= rows {
            return Err(invalid!(RowPastEnd, row = row, rows = rows));
        }
        let field = self
            .schema_ref()
            .field(column.position(self.schema_ref())?)
            .clone();
        let value = value.into_value();
        let options = CastOptions {
            safe: false,
            ..Default::default()
        };
        let value = cast_with_options(&value, field.data_type(), &options)
            .map_err(|_| invalid!(EditType, column = field.name()))?;
        if value.null_count() > 0 && !field.is_nullable() {
            return Err(invalid!(NotNullableRow, column = field.name(), row = row));
        }
        // Edits in a row share one log, over the input of the first.
        let (base, cells) = match self.node.effect() {
            Effect::Cells(cells) => (
                self.node
                    .previous()
                    .cloned()
                    .unwrap_or_else(|| self.node.root()),
                cells.clone(),
            ),
            _ => (self.node.clone(), CellView::new()),
        };
        let cells = cells.with(field.name(), row, value);
        let bytes = cells.bytes();
        let node = base.push(Arc::new(SetCells(cells)), &self.context())?;
        let frame = self.changed_to(node);
        match bytes > crate::memory::cell_edits_limit() {
            true => frame.group_cell_edits(),
            false => Ok(frame),
        }
    }

    /// The last cell edits as a snapshot of the columns they touch, undone
    /// as one change.
    fn group_cell_edits(&self) -> Result<Self> {
        let Effect::Cells(cells) = self.node.effect() else {
            return Ok(self.clone());
        };
        let base = self
            .node
            .previous()
            .cloned()
            .unwrap_or_else(|| self.node.root());
        let mut columns: Vec<usize> = cells
            .cells()
            .iter()
            .filter_map(|(name, _)| self.schema_ref().index_of(name).ok())
            .collect();
        columns.sort_unstable();
        columns.dedup();
        let schema = Arc::new(self.schema_ref().project(&columns)?);
        let batches = read_all(&self.node, &columns, self.reporting(Task::Computing))?;
        // When deleted for disk space, read again through the edits.
        let edits = self.node.clone();
        let rebuild = move || read_all(&edits, &columns, None);
        let table = Table::with_rebuild(schema, batches, Some(Box::new(rebuild)))?;
        let snapshot = base.push(Arc::new(Snapshot(table)), &self.context())?;
        // The edits' own undo entries go: the frame before them is undone to.
        let history = self.history.as_ref().map(|h| {
            h.without_top(|n| {
                matches!(n.effect(), Effect::Cells(_))
                    && n.previous().is_some_and(|p| Arc::ptr_eq(p, &base))
            })
        });
        Ok(Self {
            node: snapshot,
            history,
            ..self.clone()
        })
    }
}

/// Every row of `columns` of `node`'s output.
fn read_all(
    node: &Arc<Node>,
    columns: &[usize],
    progress: Option<(&Progress, Task)>,
) -> Result<Vec<RecordBatch>> {
    Ok(node
        .read_parallel(Some(columns.to_vec()), progress, |batches, _| Ok(batches))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}
