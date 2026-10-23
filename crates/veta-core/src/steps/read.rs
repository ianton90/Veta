//! Reading a step's output: the rows of a window, with all columns or only
//! some (#171): each step reads from the one before only what it needs.

use std::ops::Range;

use arrow::array::{ArrayRef, new_null_array};
use arrow::compute::concat_batches;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::compute::Cache;
use super::pipeline::Pipeline;
use super::transform::Input;
use crate::error::Result;

impl Pipeline {
    /// Reads rows of level `level`; `range` must be within it.
    pub(super) fn read_level(&self, level: usize, range: Range<usize>) -> Result<RecordBatch> {
        if level == 0 {
            return self.source.read(range);
        }
        let schema = self.levels[level].schema.clone();
        let previous = level - 1;
        if let Some(cache) = &self.caches[previous]
            && !matches!(cache.as_ref(), Cache::Checked(_))
        {
            return match cache.as_ref() {
                Cache::Rows(rows) => {
                    let parts =
                        read_kept(&rows.map(range), |span| self.read_level(previous, span))?;
                    concat(&schema, parts)
                }
                Cache::Data(source) => source.read(range),
                Cache::Project(_) => {
                    let batch = self.read_level(previous, range)?;
                    let input = batch.schema();
                    let columns = schema
                        .fields()
                        .iter()
                        .map(|f| Ok(batch.column(input.index_of(f.name())?).clone()))
                        .collect::<Result<Vec<_>>>()?;
                    with_schema(schema, columns, batch.num_rows())
                }
                // Read window by window below.
                Cache::Checked(_) => unreachable!("handled above"),
            };
        }
        let input = Input {
            pipeline: self,
            level: previous,
        };
        self.steps[previous].with_transform(|t| t.read(&input, range, None))
    }

    /// Like [`Self::read_level`], with only the columns `names` (in the
    /// level's order). Each step reads from the one before only what these
    /// need: rows map through row steps, names through renames, and columns
    /// a step doesn't compute pass through it. Only when a wanted column is
    /// computed from others is the step's whole input read.
    pub(super) fn read_level_columns(
        &self,
        level: usize,
        range: Range<usize>,
        names: &[String],
    ) -> Result<RecordBatch> {
        if level == 0 {
            return self.source.read_columns(range, names);
        }
        let schema = project_schema(&self.levels[level].schema, names);
        let previous = level - 1;
        let read = |names: &[String], range| self.read_level_columns(previous, range, names);
        if let Some(cache) = &self.caches[previous]
            && !matches!(cache.as_ref(), Cache::Checked(_))
        {
            return match cache.as_ref() {
                Cache::Rows(rows) => {
                    let parts = read_kept(&rows.map(range), |span| read(names, span))?;
                    concat(&schema, parts)
                }
                Cache::Data(source) => source.read_columns(range, names),
                Cache::Project(_) => {
                    let batch = read(names, range)?;
                    let input = batch.schema();
                    let columns = schema
                        .fields()
                        .iter()
                        .map(|f| Ok(batch.column(input.index_of(f.name())?).clone()))
                        .collect::<Result<Vec<_>>>()?;
                    with_schema(schema, columns, batch.num_rows())
                }
                Cache::Checked(_) => unreachable!("handled above"),
            };
        }
        let input = Input {
            pipeline: self,
            level: previous,
        };
        self.steps[previous].with_transform(|t| t.read(&input, range, Some(names)))
    }
}

/// Rows skipped between runs that are still read as one span (see
/// [`span_groups`]).
const SPAN_GAP: usize = 4096;

/// Groups input row runs (in order) into spans to read at once: runs less
/// than [`SPAN_GAP`] rows apart share a span. Returns each span with its runs.
fn span_groups(runs: &[Range<usize>]) -> Vec<(Range<usize>, Vec<Range<usize>>)> {
    let mut groups: Vec<(Range<usize>, Vec<Range<usize>>)> = Vec::new();
    for run in runs {
        match groups.last_mut() {
            Some((span, members)) if run.start <= span.end + SPAN_GAP => {
                span.end = run.end;
                members.push(run.clone());
            }
            _ => groups.push((run.clone(), vec![run.clone()])),
        }
    }
    groups
}

/// The rows in `runs` (input rows kept by a filter, in order), read by
/// `read`. Kept rows close together are read as one span and picked out with
/// a mask: a filter keeping scattered rows has millions of short runs, too
/// many to read one by one.
fn read_kept(
    runs: &[Range<usize>],
    read: impl Fn(Range<usize>) -> Result<RecordBatch>,
) -> Result<Vec<RecordBatch>> {
    span_groups(runs)
        .into_iter()
        .map(|(span, runs)| {
            let batch = read(span.clone())?;
            if runs.len() == 1 && runs[0] == span {
                return Ok(batch);
            }
            let mut mask = arrow::array::BooleanBufferBuilder::new(span.len());
            mask.append_n(span.len(), false);
            for run in runs {
                for i in run.start - span.start..run.end - span.start {
                    mask.set_bit(i, true);
                }
            }
            let mask = arrow::array::BooleanArray::new(mask.finish(), None);
            Ok(arrow::compute::filter_record_batch(&batch, &mask)?)
        })
        .collect()
}

/// The fields of `schema` named in `names`, in `schema`'s order.
pub(super) fn project_schema(schema: &SchemaRef, names: &[String]) -> SchemaRef {
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .filter(|f| names.contains(f.name()))
        .cloned()
        .collect();
    std::sync::Arc::new(arrow::datatypes::Schema::new_with_metadata(
        fields,
        schema.metadata().clone(),
    ))
}

/// A batch of `rows` rows with every value null.
pub(super) fn empty_rows(schema: &SchemaRef, rows: usize) -> Result<RecordBatch> {
    let columns = schema
        .fields()
        .iter()
        .map(|f| new_null_array(f.data_type(), rows))
        .collect();
    with_schema(schema.clone(), columns, rows)
}

pub(crate) fn with_schema(
    schema: SchemaRef,
    columns: Vec<ArrayRef>,
    rows: usize,
) -> Result<RecordBatch> {
    let options = RecordBatchOptions::new().with_row_count(Some(rows));
    Ok(RecordBatch::try_new_with_options(
        schema, columns, &options,
    )?)
}

/// Concatenates parts, giving them `schema` (parts may differ only in
/// nullability).
pub(super) fn concat(schema: &SchemaRef, parts: Vec<RecordBatch>) -> Result<RecordBatch> {
    let parts = parts
        .into_iter()
        .map(|b| {
            let rows = b.num_rows();
            with_schema(schema.clone(), b.columns().to_vec(), rows)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(concat_batches(schema, &parts)?)
}

#[cfg(test)]
mod span_tests {
    use super::*;

    #[test]
    fn runs_close_together_share_a_span() {
        let runs = [0..2, 5..6, 4000..4001, 20_000..20_010, 20_011..20_012];
        let groups = span_groups(&runs);
        let spans: Vec<_> = groups.iter().map(|(s, _)| s.clone()).collect();
        assert_eq!(spans, [0..4001, 20_000..20_012]);
        assert_eq!(groups[0].1, [0..2, 5..6, 4000..4001]);
        assert!(span_groups(&[]).is_empty());
    }
}
