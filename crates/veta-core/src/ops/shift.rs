//! Shifting columns' values down (or up) by some rows.

use std::ops::Range;
use std::sync::Arc;

use arrow::array::{ArrayRef, new_null_array};
use arrow::compute::concat;

use super::Columns;
use crate::error::Result;
use crate::plan::{Col, Context, Effect, Node, Planned, Step};

/// Moves the values of `columns` down by `rows` (up when negative); the
/// rows left empty become null.
#[derive(Debug)]
pub(crate) struct Shift {
    pub(crate) columns: Columns,
    pub(crate) rows: i64,
}

impl Step for Shift {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        let shifted = self.columns.positions(input.schema())?;
        let mut new = 0;
        let map = (0..input.schema().fields().len())
            .map(|i| match shifted.contains(&i) {
                true => {
                    new += 1;
                    Col::New(new - 1)
                }
                false => Col::Input(i),
            })
            .collect();
        let fields: Vec<_> = input
            .schema()
            .fields()
            .iter()
            .enumerate()
            .map(|(i, f)| match shifted.contains(&i) {
                true => f.as_ref().clone().with_nullable(true),
                false => f.as_ref().clone(),
            })
            .collect();
        let schema =
            arrow::datatypes::Schema::new_with_metadata(fields, input.schema().metadata().clone());
        Ok(Planned {
            schema: Arc::new(schema),
            effect: Effect::Derived(map),
            file: None,
        })
    }

    /// Rows `rows` of the shifted columns: the input's rows `rows.len()`
    /// above (or below), nulls where there are none.
    fn derive(&self, input: &Node, rows: Range<usize>) -> Result<Vec<ArrayRef>> {
        let mut shifted = self.columns.positions(input.schema())?;
        shifted.sort_unstable();
        let total = input.rows()? as i64;
        let (start, end) = (rows.start as i64 - self.rows, rows.end as i64 - self.rows);
        let (from, to) = (start.clamp(0, total), end.clamp(0, total));
        let batch = input.read(from as usize..to as usize, &shifted)?;
        let (before, after) = ((from - start) as usize, (end - to) as usize);
        batch
            .columns()
            .iter()
            .map(|column| {
                let data_type = column.data_type();
                let parts = [
                    new_null_array(data_type, before),
                    column.clone(),
                    new_null_array(data_type, after),
                ];
                let parts: Vec<&dyn arrow::array::Array> =
                    parts.iter().map(|a| a.as_ref()).collect();
                Ok(concat(&parts)?)
            })
            .collect()
    }
}
