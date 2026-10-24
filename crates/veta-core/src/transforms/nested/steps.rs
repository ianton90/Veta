//! The nested value steps: expand a struct into columns, explode a list
//! into rows, add or remove struct fields, collect rows into lists and
//! combine columns into a struct.

use std::ops::Range;
use std::sync::Arc;

use arrow::datatypes::{DataType, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;

use super::*;
use crate::display::PathPart;
use crate::error::{Error, Result, invalid};
use crate::source::{DataSource, MemorySource, SourceMode};
use crate::steps::{Cache, ComputeJob, Input, Level, PASS_ROWS, Transform, compute_first};
use crate::transforms::sort::{Sort, SortKey, sort_external, sort_in_memory};

/// [`Step::ExpandStruct`](crate::steps::Step::ExpandStruct).
pub(crate) struct ExpandStruct<'a> {
    pub(crate) column: &'a str,
    pub(crate) all_levels: bool,
}

impl Transform for ExpandStruct<'_> {
    fn level(&self, input: &Level) -> Result<Level> {
        let (_, schema) = expand_all(&input.schema, self.column, self.all_levels)?;
        let fields = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn apply(&self, batch: &RecordBatch, _: usize, output: &SchemaRef) -> Result<RecordBatch> {
        expand_levels(batch, self.column, self.all_levels, output)
    }
}

/// [`Step::AddField`](crate::steps::Step::AddField).
pub(crate) struct AddField<'a> {
    pub(crate) column: &'a str,
    pub(crate) path: &'a [PathPart],
    pub(crate) name: &'a str,
    pub(crate) data_type: &'a DataType,
}

impl Transform for AddField<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (column != self.column).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        let fields = change_field_fields(
            &input.schema,
            self.column,
            self.path,
            self.name,
            Some(self.data_type),
        )?;
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn apply(&self, batch: &RecordBatch, _: usize, output: &SchemaRef) -> Result<RecordBatch> {
        change_field(batch, self.column, output)
    }

    fn changed_columns(&self) -> Vec<&str> {
        vec![self.column]
    }
}

/// [`Step::RemoveField`](crate::steps::Step::RemoveField).
pub(crate) struct RemoveField<'a> {
    pub(crate) column: &'a str,
    pub(crate) path: &'a [PathPart],
    pub(crate) name: &'a str,
}

impl Transform for RemoveField<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (column != self.column).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        let fields = change_field_fields(&input.schema, self.column, self.path, self.name, None)?;
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn apply(&self, batch: &RecordBatch, _: usize, output: &SchemaRef) -> Result<RecordBatch> {
        change_field(batch, self.column, output)
    }

    fn changed_columns(&self) -> Vec<&str> {
        vec![self.column]
    }
}

/// [`Step::CombineStruct`](crate::steps::Step::CombineStruct).
pub(crate) struct CombineStruct<'a> {
    pub(crate) columns: &'a [String],
    pub(crate) name: &'a str,
}

impl Transform for CombineStruct<'_> {
    fn level(&self, input: &Level) -> Result<Level> {
        let fields = combine_fields(&input.schema, self.columns, self.name)?;
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn apply(&self, batch: &RecordBatch, _: usize, output: &SchemaRef) -> Result<RecordBatch> {
        combine(batch, self.columns, self.name, output)
    }
}

/// [`Step::ExplodeList`](crate::steps::Step::ExplodeList).
pub(crate) struct ExplodeList<'a> {
    pub(crate) column: &'a str,
}

impl Transform for ExplodeList<'_> {
    fn level(&self, input: &Level) -> Result<Level> {
        explode_fields(&input.schema, self.column)?;
        Ok(input.clone())
    }

    fn full_pass(&self) -> bool {
        true
    }

    fn read(&self, _: &Input<'_>, _: Range<usize>, _: Option<&[String]>) -> Result<RecordBatch> {
        compute_first()
    }

    /// One row per list item, batch by batch.
    fn compute(
        &self,
        job: &ComputeJob,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Cache> {
        let column = self.column;
        let input = job.input_schema()?;
        let schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
            explode_fields(&input, column)?,
            input.metadata().clone(),
        ));
        let mut start = 0;
        let mut produce = || -> Option<Result<RecordBatch>> {
            if start >= input_rows {
                return None;
            }
            let end = (start + PASS_ROWS).min(input_rows);
            let result = job
                .read(start..end)
                .and_then(|batch| explode(&batch, column, &schema));
            start = end;
            if result.is_ok() && !progress(start as f32 / input_rows as f32) {
                return Some(Err(Error::Cancelled));
            }
            Some(result)
        };
        Ok(Cache::Data(job.materialize(schema.clone(), &mut produce)?))
    }
}

/// [`Step::CollectList`](crate::steps::Step::CollectList).
pub(crate) struct CollectList<'a> {
    pub(crate) keys: &'a [String],
    pub(crate) columns: &'a [String],
}

impl Transform for CollectList<'_> {
    fn level(&self, input: &Level) -> Result<Level> {
        collect_fields(&input.schema, self.keys, self.columns)?;
        Ok(input.clone())
    }

    fn full_pass(&self) -> bool {
        true
    }

    fn read(&self, _: &Input<'_>, _: Range<usize>, _: Option<&[String]>) -> Result<RecordBatch> {
        compute_first()
    }

    /// One row per group of equal keys, the other columns collected into
    /// lists. Rows are sorted by the keys first (stable, so items keep their
    /// order), which also orders the groups.
    fn compute(
        &self,
        job: &ComputeJob,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Cache> {
        let (keys, columns) = (self.keys, self.columns);
        let input = job.input_schema()?;
        let schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
            collect_fields(&input, keys, columns)?,
            input.metadata().clone(),
        ));
        let read = |range| job.read(range);
        let sorted: Option<Arc<dyn DataSource>> = if keys.is_empty() {
            None
        } else {
            let sort = Sort {
                keys: keys
                    .iter()
                    .map(|k| SortKey {
                        column: k.clone(),
                        descending: false,
                        nulls_first: false,
                    })
                    .collect(),
            };
            let mut sort_progress = |f: f32| progress(0.5 * f);
            Some(match job.mode() {
                SourceMode::InMemory => {
                    let batch = sort_in_memory(
                        &sort,
                        &input,
                        input_rows,
                        &read,
                        PASS_ROWS,
                        &mut sort_progress,
                    )?;
                    Arc::new(MemorySource::new(input.clone(), vec![batch])?)
                }
                SourceMode::Paged => Arc::new(sort_external(
                    &sort,
                    &input,
                    input_rows,
                    &read,
                    &|range| job.read_columns(range, keys),
                    PASS_ROWS,
                    None,
                    &mut sort_progress,
                )?),
            })
        };
        let read_sorted = |range: Range<usize>| match &sorted {
            Some(source) => source.read(range),
            None => read(range),
        };
        let key_columns = keys
            .iter()
            .map(|k| input.index_of(k))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let converter = if keys.is_empty() {
            None
        } else {
            let fields = key_columns
                .iter()
                .map(|&i| arrow::row::SortField::new(input.field(i).data_type().clone()))
                .collect();
            Some(arrow::row::RowConverter::new(fields)?)
        };

        // Groups can span reads: the rows of the last, maybe unfinished,
        // group wait in `pending` (as read, joined once the group ends) for
        // the next read. Without keys the whole input is one group.
        let mut start = 0;
        let mut pending: Vec<RecordBatch> = Vec::new();
        // Whether row `b` of `next` has the same keys as the last pending row.
        let continues = |pending: &[RecordBatch], next: &RecordBatch| -> Result<bool> {
            let (Some(last), Some(converter)) = (pending.last(), converter.as_ref()) else {
                return Ok(!pending.is_empty());
            };
            let keys = |batch: &RecordBatch, row: usize| -> Vec<arrow::array::ArrayRef> {
                key_columns
                    .iter()
                    .map(|&i| batch.column(i).slice(row, 1))
                    .collect()
            };
            let a = converter.convert_columns(&keys(last, last.num_rows() - 1))?;
            let b = converter.convert_columns(&keys(next, 0))?;
            Ok(a.row(0) == b.row(0))
        };
        let mut next = |progress: &mut dyn FnMut(f32) -> bool| -> Result<Option<RecordBatch>> {
            loop {
                let end = (start + PASS_ROWS).min(input_rows);
                let read = (start < end).then(|| read_sorted(start..end)).transpose()?;
                start = end;
                let last = start >= input_rows;
                if !progress(0.5 + 0.5 * start as f32 / input_rows.max(1) as f32) {
                    return Err(Error::Cancelled);
                }
                let read = match read {
                    Some(read) if read.num_rows() > 0 => read,
                    _ if pending.is_empty() => return Ok(None),
                    // The end: the pending group is complete.
                    _ => {
                        let rows: usize = pending.iter().map(RecordBatch::num_rows).sum();
                        let group = arrow::compute::concat_batches(&input, &pending)?;
                        pending.clear();
                        return group_lists(&group, &[0, rows], keys, &schema).map(Some);
                    }
                };
                let starts = group_starts(&read, &key_columns, converter.as_ref())?;
                let joins = continues(&pending, &read)?;
                if joins && starts.len() == 1 && !last {
                    // All of it belongs to the pending group.
                    pending.push(read);
                    continue;
                }
                // Complete: the pending group and this read's groups up to its
                // last one (which may go on in the next read).
                let cut = match last {
                    true => read.num_rows(),
                    false => *starts.last().expect("rows read"),
                };
                let offset: usize = pending.iter().map(RecordBatch::num_rows).sum();
                let mut bounds: Vec<usize> = if pending.is_empty() { vec![] } else { vec![0] };
                bounds.extend(
                    starts
                        .iter()
                        .filter(|&&s| s < cut && !(joins && s == 0))
                        .map(|s| offset + s),
                );
                bounds.push(offset + cut);
                pending.push(read.slice(0, cut));
                let complete = arrow::compute::concat_batches(&input, &pending)?;
                pending.clear();
                if !last {
                    pending.push(read.slice(cut, read.num_rows() - cut));
                }
                if complete.num_rows() > 0 {
                    return group_lists(&complete, &bounds, keys, &schema).map(Some);
                }
            }
        };
        let mut produce = || next(progress).transpose();
        Ok(Cache::Data(job.materialize(schema.clone(), &mut produce)?))
    }
}

/// [`collect_groups`], refusing a group too large for one list (2^31
/// values).
fn group_lists(
    batch: &RecordBatch,
    bounds: &[usize],
    keys: &[String],
    schema: &Arc<Schema>,
) -> Result<RecordBatch> {
    if batch.num_rows() > i32::MAX as usize {
        return Err(invalid!(CollectTooLarge));
    }
    collect_groups(batch, bounds, keys, schema)
}
