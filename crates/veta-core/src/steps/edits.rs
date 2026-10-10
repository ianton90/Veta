use std::collections::BTreeMap;

use arrow::array::{Array, ArrayRef};
use arrow::compute::interleave;
use arrow::record_batch::RecordBatch;

use super::invalid;
use super::level::Level;
use crate::error::Result;

/// Edited cell values: column name → row → new value (a one-element array of
/// the column's type, possibly null).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellEdits {
    columns: BTreeMap<String, BTreeMap<usize, ArrayRef>>,
}

impl CellEdits {
    pub fn get(&self, column: &str, row: usize) -> Option<&ArrayRef> {
        self.columns.get(column)?.get(&row)
    }

    /// Sets a value and returns the one it replaced in this step, if any.
    pub fn set(&mut self, column: &str, row: usize, value: ArrayRef) -> Option<ArrayRef> {
        self.columns
            .entry(column.to_owned())
            .or_default()
            .insert(row, value)
    }

    /// Removes an edit and returns it.
    pub fn remove(&mut self, column: &str, row: usize) -> Option<ArrayRef> {
        let rows = self.columns.get_mut(column)?;
        let value = rows.remove(&row);
        if rows.is_empty() {
            self.columns.remove(column);
        }
        value
    }

    /// Number of edited cells.
    pub fn len(&self) -> usize {
        self.columns.values().map(BTreeMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub(super) fn validate(&self, level: &Level) -> Result<()> {
        for (column, rows) in &self.columns {
            let field = level.field(column)?;
            if let Some((&row, _)) = rows.range(level.num_rows..).next() {
                return Err(invalid(format!(
                    "row {} is past the last row ({})",
                    row + 1,
                    level.num_rows
                )));
            }
            for value in rows.values() {
                if value.data_type() != field.data_type() || value.len() != 1 {
                    return Err(invalid(format!(
                        "edited value does not match the type of {column}"
                    )));
                }
                if value.is_null(0) && !field.is_nullable() {
                    return Err(invalid(format!("{column} cannot be empty")));
                }
            }
        }
        Ok(())
    }

    /// Applies the edits to `batch`, whose first row is row `start`.
    pub(super) fn apply(&self, batch: &RecordBatch, start: usize) -> Result<RecordBatch> {
        let schema = batch.schema();
        let window = start..start + batch.num_rows();
        let mut columns = batch.columns().to_vec();
        for (i, field) in schema.fields().iter().enumerate() {
            let Some(rows) = self.columns.get(field.name()) else {
                continue;
            };
            let edits: Vec<(usize, &ArrayRef)> =
                rows.range(window.clone()).map(|(r, v)| (*r, v)).collect();
            if edits.is_empty() {
                continue;
            }
            let mut arrays: Vec<&dyn Array> = vec![columns[i].as_ref()];
            let mut indices: Vec<(usize, usize)> = (0..batch.num_rows()).map(|r| (0, r)).collect();
            for (k, (row, value)) in edits.into_iter().enumerate() {
                arrays.push(value.as_ref());
                indices[row - start] = (k + 1, 0);
            }
            columns[i] = interleave(&arrays, &indices)?;
        }
        Ok(RecordBatch::try_new(schema, columns)?)
    }
}
