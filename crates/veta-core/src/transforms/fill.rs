//! Filling nulls with the nearest value above or below.

use arrow::array::{Array, ArrayRef, UInt32Array, new_null_array};
use arrow::compute::{concat, take};
use arrow::datatypes::Schema;

use crate::error::Result;
use crate::steps::invalid;

/// Fills nulls in `columns` with the previous non-null value (down) or the
/// next one (up).
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub columns: Vec<String>,
    pub up: bool,
}

impl Fill {
    pub fn describe(&self) -> String {
        let direction = if self.up { "up" } else { "down" };
        match self.columns.as_slice() {
            [one] => format!("Filled {direction} {one}"),
            many => format!("Filled {direction} {} columns", many.len()),
        }
    }

    pub(crate) fn validate(&self, schema: &Schema) -> Result<()> {
        if self.columns.is_empty() {
            return Err(invalid("no columns to fill".into()));
        }
        for name in &self.columns {
            schema
                .index_of(name)
                .map_err(|_| invalid(format!("no column named {name:?}")))?;
        }
        Ok(())
    }
}

/// The value carried into the next batch: the last non-null value of
/// `column` (first, when filling up), or `carry` if there is none.
pub(crate) fn carry_out(column: &ArrayRef, carry: Option<ArrayRef>, up: bool) -> Option<ArrayRef> {
    let n = column.len();
    let found = if up {
        (0..n).find(|&i| column.is_valid(i))
    } else {
        (0..n).rev().find(|&i| column.is_valid(i))
    };
    match found {
        Some(i) => Some(column.slice(i, 1)),
        None => carry,
    }
}

/// Fills nulls in `column`, using `carry` (one value) for nulls before the
/// first value (after the last one, when filling up).
pub(crate) fn fill(column: &ArrayRef, carry: Option<&ArrayRef>, up: bool) -> Result<ArrayRef> {
    let n = column.len();
    // Index 0 is the carried value; row i is index i + 1.
    let first = match carry {
        Some(c) => c.clone(),
        None => new_null_array(column.data_type(), 1),
    };
    let values = concat(&[first.as_ref(), column.as_ref()])?;
    let mut indices: Vec<Option<u32>> = Vec::with_capacity(n);
    let mut current = carry.map(|_| 0u32);
    let rows: Box<dyn Iterator<Item = usize>> = if up {
        Box::new((0..n).rev())
    } else {
        Box::new(0..n)
    };
    for i in rows {
        if column.is_valid(i) {
            current = Some(i as u32 + 1);
        }
        indices.push(current);
    }
    if up {
        indices.reverse();
    }
    Ok(take(&values, &UInt32Array::from(indices), None)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int64Array;
    use std::sync::Arc;

    fn ints(a: &ArrayRef) -> Vec<Option<i64>> {
        a.as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .iter()
            .collect()
    }

    #[test]
    fn fills_within_and_across_batches() {
        let column: ArrayRef = Arc::new(Int64Array::from(vec![None, Some(1), None, Some(2), None]));
        assert_eq!(
            ints(&fill(&column, None, false).unwrap()),
            [None, Some(1), Some(1), Some(2), Some(2)]
        );
        assert_eq!(
            ints(&fill(&column, None, true).unwrap()),
            [Some(1), Some(1), Some(2), Some(2), None]
        );
        let carry: ArrayRef = Arc::new(Int64Array::from(vec![9]));
        assert_eq!(
            ints(&fill(&column, Some(&carry), false).unwrap())[0],
            Some(9)
        );
        assert_eq!(
            ints(&fill(&column, Some(&carry), true).unwrap())[4],
            Some(9)
        );

        let empty: ArrayRef = Arc::new(Int64Array::from(vec![None, None]));
        assert_eq!(
            carry_out(&empty, Some(carry.clone()), false).map(|c| ints(&c)),
            Some(vec![Some(9)])
        );
        assert_eq!(
            carry_out(&column, None, false).map(|c| ints(&c)),
            Some(vec![Some(2)])
        );
        assert_eq!(
            carry_out(&column, None, true).map(|c| ints(&c)),
            Some(vec![Some(1)])
        );
    }
}
