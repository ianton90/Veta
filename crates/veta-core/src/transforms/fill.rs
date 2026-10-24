//! Filling nulls with the nearest value above or below.

use crate::error::invalid;
use arrow::array::{Array, ArrayRef, UInt32Array, new_null_array};
use arrow::compute::{concat, take};
use arrow::datatypes::Schema;

use std::ops::Range;
use std::sync::Arc;

use arrow::record_batch::RecordBatch;

use crate::error::{Error, Result};
use crate::steps::{Cache, ComputeJob, Input, Level, PASS_ROWS, Transform, compute_first};

/// Fills nulls in `columns` with the previous non-null value (down) or the
/// next one (up).
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub columns: Vec<String>,
    pub up: bool,
}

impl Fill {
    pub(crate) fn validate(&self, schema: &Schema) -> Result<()> {
        if self.columns.is_empty() {
            return Err(invalid!(NoColumnsToFill));
        }
        for name in &self.columns {
            schema
                .index_of(name)
                .map_err(|_| invalid!(ColumnNotFound, column = name))?;
        }
        Ok(())
    }
}

impl Transform for Fill {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (!self.columns.iter().any(|c| c == column)).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        self.validate(&input.schema)?;
        Ok(input.clone())
    }

    fn full_pass(&self) -> bool {
        true
    }

    fn read(&self, _: &Input<'_>, _: Range<usize>, _: Option<&[String]>) -> Result<RecordBatch> {
        compute_first()
    }

    /// Fills nulls batch by batch, carrying the last value across batches.
    /// Filling up first finds, from the end, the value carried into each
    /// batch.
    fn compute(
        &self,
        job: &ComputeJob,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Cache> {
        let schema = job.input_schema()?;
        let f = self;
        let indices = f
            .columns
            .iter()
            .map(|c| schema.index_of(c))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let batches = input_rows.div_ceil(PASS_ROWS);
        let up = f.up;

        // First, only the filled columns: each batch's value to carry on
        // (its last value, or its first when filling up), on all cores.
        let columns = f.columns.clone();
        let pieces = job.read_parallel(
            input_rows,
            Some(f.columns.clone()),
            move |batches, first| {
                let carries = batches
                    .iter()
                    .map(|batch| {
                        columns
                            .iter()
                            .map(|c| {
                                let column = batch
                                    .column_by_name(c)
                                    .ok_or_else(|| invalid!(ColumnNotFound, column = c))?;
                                carry_out(column, None, up)
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .collect::<Result<Vec<_>>>()?;
                let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
                Ok((carries, first + rows))
            },
        );
        let mut found: Vec<Vec<Option<ArrayRef>>> = Vec::with_capacity(batches);
        for piece in pieces {
            let (carries, end) = piece?;
            found.extend(carries);
            if !progress(0.3 * end as f32 / input_rows.max(1) as f32) {
                return Err(Error::Cancelled);
            }
        }
        // The value carried into each batch: the nearest found before it
        // (after it, when filling up).
        let mut incoming: Vec<Vec<Option<ArrayRef>>> = vec![vec![None; indices.len()]; batches];
        let mut carry: Vec<Option<ArrayRef>> = vec![None; indices.len()];
        let order: Box<dyn Iterator<Item = usize>> = match up {
            true => Box::new((0..batches).rev()),
            false => Box::new(0..batches),
        };
        for b in order {
            incoming[b] = carry.clone();
            for (k, value) in found[b].iter().enumerate() {
                if value.is_some() {
                    carry[k] = value.clone();
                }
            }
        }

        // Then every column, each batch filled on its own, on all cores.
        let incoming = Arc::new(incoming);
        let pieces = job.read_parallel(input_rows, None, move |batches, first| {
            let mut start = first;
            let mut filled = Vec::with_capacity(batches.len());
            for batch in batches {
                let carries = &incoming[start / PASS_ROWS];
                start += batch.num_rows();
                let mut columns = batch.columns().to_vec();
                for (k, &i) in indices.iter().enumerate() {
                    columns[i] = fill(&columns[i], carries[k].as_ref(), up)?;
                }
                filled.push(RecordBatch::try_new(batch.schema(), columns)?);
            }
            Ok((filled, start))
        });
        let mut pieces = pieces.into_iter();
        let mut ready = std::collections::VecDeque::new();
        let mut produce = || -> Option<Result<RecordBatch>> {
            loop {
                if let Some(batch) = ready.pop_front() {
                    return Some(Ok(batch));
                }
                match pieces.next()? {
                    Ok((filled, end)) => {
                        if !progress(0.3 + 0.7 * end as f32 / input_rows.max(1) as f32) {
                            return Some(Err(Error::Cancelled));
                        }
                        ready.extend(filled);
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
        };
        Ok(Cache::Data(job.materialize(schema.clone(), &mut produce)?))
    }
}

/// The value carried into the next batch: the last non-null value of
/// `column` (first, when filling up), or `carry` if there is none. A copy:
/// a slice would keep the whole batch in memory while the value is kept.
pub(crate) fn carry_out(
    column: &ArrayRef,
    carry: Option<ArrayRef>,
    up: bool,
) -> Result<Option<ArrayRef>> {
    let n = column.len();
    let found = if up {
        (0..n).find(|&i| column.is_valid(i))
    } else {
        (0..n).rev().find(|&i| column.is_valid(i))
    };
    Ok(match found {
        Some(i) => Some(take(column, &UInt32Array::from(vec![i as u32]), None)?),
        None => carry,
    })
}

/// Fills nulls in `column`, using `carry` (one value) for nulls before the
/// first value (after the last one, when filling up).
pub(crate) fn fill(column: &ArrayRef, carry: Option<&ArrayRef>, up: bool) -> Result<ArrayRef> {
    if column.null_count() == 0 {
        return Ok(column.clone());
    }
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
            carry_out(&empty, Some(carry.clone()), false)
                .unwrap()
                .map(|c| ints(&c)),
            Some(vec![Some(9)])
        );
        assert_eq!(
            carry_out(&column, None, false).unwrap().map(|c| ints(&c)),
            Some(vec![Some(2)])
        );
        assert_eq!(
            carry_out(&column, None, true).unwrap().map(|c| ints(&c)),
            Some(vec![Some(1)])
        );

        // The carry is a copy, not a view of the whole batch; a batch without
        // nulls comes back as it is.
        let big: ArrayRef = Arc::new(Int64Array::from_iter_values(0..100_000));
        let carry = carry_out(&big, None, false).unwrap().unwrap();
        assert!(carry.get_array_memory_size() < 1024);
        assert!(Arc::ptr_eq(&fill(&big, None, false).unwrap(), &big));
    }
}
