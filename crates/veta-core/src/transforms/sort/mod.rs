//! Sorting rows by one or more columns.
//!
//! Small (in-memory) inputs are sorted in memory. Paged inputs are sorted
//! by key range ([`buckets`]): rows go to buckets of keys between values
//! sampled from the input, and the buckets are sorted on all cores and
//! written out in order, into a checkpoint file that later steps read on
//! demand, or straight into the saved file.

mod buckets;

use crate::error::invalid;
use arrow::array::{ArrayRef, UInt32Array};
use arrow::compute::{SortOptions, concat_batches, take};
use arrow::datatypes::{Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use arrow::row::{RowConverter, Rows, SortField};

use crate::error::{Error, Result};
use crate::steps::checkpoint::{Checkpoint, TempFile, write_parquet};

pub(crate) use buckets::{Plan, Sorted};

/// Sorts rows by `keys`, the first key first. The sort is stable.
#[derive(Debug, Clone, PartialEq)]
pub struct Sort {
    pub keys: Vec<SortKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub column: String,
    pub descending: bool,
    /// Nulls before other values (otherwise after).
    pub nulls_first: bool,
}

/// Rows per sorted batch.
const OUTPUT_ROWS: usize = 64 * 1024;
/// Rows per slice when converting sort keys on several cores.
const SLICE_ROWS: usize = 64 * 1024;

impl Sort {
    /// Checks the sort against the input schema.
    pub(crate) fn validate(&self, schema: &Schema) -> Result<()> {
        self.converter(schema).map(|_| ())
    }

    fn columns(&self, schema: &Schema) -> Result<Vec<usize>> {
        if self.keys.is_empty() {
            return Err(invalid!(SortNoColumns));
        }
        self.keys
            .iter()
            .map(|k| {
                schema
                    .index_of(&k.column)
                    .map_err(|_| invalid!(ColumnNotFound, column = k.column))
            })
            .collect()
    }

    fn converter(&self, schema: &Schema) -> Result<RowConverter> {
        let fields = self
            .columns(schema)?
            .into_iter()
            .zip(&self.keys)
            .map(|(i, k)| {
                let options = SortOptions {
                    descending: k.descending,
                    nulls_first: k.nulls_first,
                };
                SortField::new_with_options(schema.field(i).data_type().clone(), options)
            })
            .collect::<Vec<_>>();
        if !RowConverter::supports_fields(&fields) {
            return Err(invalid!(SortUnsortable));
        }
        Ok(RowConverter::new(fields)?)
    }
}

/// Sort state shared by the in-memory and external sorts.
struct Sorter {
    /// The key columns, found by name: batches may hold only them.
    names: Vec<String>,
    converter: RowConverter,
}

impl Sorter {
    fn new(sort: &Sort, schema: &Schema) -> Result<Self> {
        sort.columns(schema)?;
        Ok(Self {
            names: sort.keys.iter().map(|k| k.column.clone()).collect(),
            converter: sort.converter(schema)?,
        })
    }

    /// The sort keys of `batch`'s rows, in the row format (compared as
    /// bytes).
    fn rows(&self, batch: &RecordBatch) -> Result<Rows> {
        let keys = self
            .names
            .iter()
            .map(|name| {
                batch
                    .column_by_name(name)
                    .cloned()
                    .ok_or_else(|| invalid!(ColumnNotFound, column = name))
            })
            .collect::<Result<Vec<ArrayRef>>>()?;
        Ok(self.converter.convert_columns(&keys)?)
    }

    /// The order of the rows of `batches`, numbered across them, that
    /// sorts them (stable). Keys are converted on all cores.
    fn order(&self, batches: &[RecordBatch]) -> Result<Vec<u32>> {
        let parts = crate::parallel::map(batches.len(), |i| self.rows(&batches[i]))?;
        Ok(sorted_order(&parts))
    }

    /// Sorts one batch (stable). Keys are converted in slices and columns
    /// reordered at the same time, on all cores.
    fn sort(&self, batch: &RecordBatch) -> Result<RecordBatch> {
        let n = batch.num_rows();
        let slices: Vec<RecordBatch> = (0..n)
            .step_by(SLICE_ROWS)
            .map(|start| batch.slice(start, SLICE_ROWS.min(n - start)))
            .collect();
        let order = UInt32Array::from(self.order(&slices)?);
        let columns = crate::parallel::map(batch.num_columns(), |c| {
            Ok(take(batch.column(c), &order, None)?)
        })?;
        let options = RecordBatchOptions::new().with_row_count(Some(n));
        Ok(RecordBatch::try_new_with_options(
            batch.schema(),
            columns,
            &options,
        )?)
    }
}

/// The order of the rows in `parts` (row-format sort keys of consecutive
/// slices) that sorts them, stable.
///
/// Ties are broken by the row's position, so an unstable sort (faster) is
/// stable. Keys of at most 16 bytes each (e.g. one number or date column)
/// are sorted as packed integers: row-format keys compare as bytes, and
/// equal-length byte strings compare like the big-endian numbers they spell.
fn sorted_order(parts: &[Rows]) -> Vec<u32> {
    let rows = || parts.iter().flat_map(Rows::iter);
    let width = rows().next().map(|r| r.data().len());
    let fixed = width.filter(|&w| w <= 16 && rows().all(|r| r.data().len() == w));
    if let Some(width) = fixed {
        let mut keys: Vec<(u128, u32)> = rows()
            .enumerate()
            .map(|(i, row)| {
                let mut bytes = [0u8; 16];
                bytes[..width].copy_from_slice(row.data());
                (u128::from_be_bytes(bytes), i as u32)
            })
            .collect();
        keys.sort_unstable();
        return keys.into_iter().map(|(_, i)| i).collect();
    }
    let mut keys: Vec<(&[u8], u32)> = rows()
        .enumerate()
        .map(|(i, row)| (row.data(), i as u32))
        .collect();
    keys.sort_unstable();
    keys.into_iter().map(|(_, i)| i).collect()
}

/// Reads input rows in `range` of the step being sorted.
pub(crate) type ReadInput<'a> = dyn Fn(std::ops::Range<usize>) -> Result<RecordBatch> + 'a;

/// Sorts all of the input in memory.
pub(crate) fn sort_in_memory(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    batch_rows: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<RecordBatch> {
    let sorter = Sorter::new(sort, schema)?;
    let batch = read_all(schema, 0..num_rows, read, batch_rows, &mut |done| {
        progress(0.9 * done as f32 / num_rows.max(1) as f32)
    })?;
    let sorted = sorter.sort(&batch)?;
    if !progress(1.0) {
        return Err(Error::Cancelled);
    }
    Ok(sorted)
}

/// Sorts the input into a temporary Parquet file. `read_keys` reads rows
/// with at least the key columns, for sampling them; `plan` sets the
/// buckets (from the data and the memory budget when `None`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sort_external(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    read_keys: &ReadInput<'_>,
    batch_rows: usize,
    plan: Option<Plan>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Checkpoint> {
    // Reading the input into buckets is half the work, sorting and writing
    // them the other half.
    let sorted = buckets::sort_buckets(
        sort,
        schema,
        num_rows,
        read,
        read_keys,
        batch_rows,
        plan,
        &mut |f| progress(0.5 * f),
    )?;
    let output = TempFile::new()?;
    let mut written = 0;
    write_parquet(
        &output,
        schema,
        sorted.map(|batch| {
            let batch = batch?;
            written += batch.num_rows();
            let fraction = written as f32 / num_rows.max(1) as f32;
            if progress(0.5 + 0.45 * fraction) {
                Ok(batch)
            } else {
                Err(Error::Cancelled)
            }
        }),
    )?;
    let checkpoint = Checkpoint::open(output, schema.clone())?;
    if !progress(1.0) {
        return Err(Error::Cancelled);
    }
    Ok(checkpoint)
}

/// Sorts the input like [`sort_external`], but returns the sorted rows, to
/// be read once in order, instead of writing them to a file.
pub(crate) fn sort_stream(
    sort: &Sort,
    schema: &SchemaRef,
    num_rows: usize,
    read: &ReadInput<'_>,
    read_keys: &ReadInput<'_>,
    batch_rows: usize,
) -> Result<Sorted> {
    buckets::sort_buckets(
        sort,
        schema,
        num_rows,
        read,
        read_keys,
        batch_rows,
        None,
        &mut |_| true,
    )
}

/// Reads `range` in batches of `batch_rows` and concatenates them.
/// `progress` gets the rows read so far and returns `false` to cancel.
fn read_all(
    schema: &SchemaRef,
    range: std::ops::Range<usize>,
    read: &ReadInput<'_>,
    batch_rows: usize,
    progress: &mut dyn FnMut(usize) -> bool,
) -> Result<RecordBatch> {
    let batches = read_batches(range, read, batch_rows, progress)?;
    Ok(concat_batches(schema, &batches)?)
}

/// Reads `range` in batches of `batch_rows`. `progress` gets the rows read
/// so far and returns `false` to cancel.
fn read_batches(
    range: std::ops::Range<usize>,
    read: &ReadInput<'_>,
    batch_rows: usize,
    progress: &mut dyn FnMut(usize) -> bool,
) -> Result<Vec<RecordBatch>> {
    let mut batches = Vec::new();
    let mut start = range.start;
    while start < range.end {
        let end = (start + batch_rows).min(range.end);
        batches.push(read(start..end)?);
        start = end;
        if !progress(start - range.start) {
            return Err(Error::Cancelled);
        }
    }
    Ok(batches)
}

#[cfg(test)]
mod tests;
