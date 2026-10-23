//! Reading a step's output: the rows of a window, with all columns or only
//! some (#171): each step reads from the one before only what it needs.

use crate::error::invalid;
use std::ops::Range;

use arrow::array::{ArrayRef, new_null_array};
use arrow::compute::concat_batches;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::Step;
use super::compute::Cache;
use super::pipeline::Pipeline;
use super::rows::kept_ranges;
use crate::error::Result;
use crate::transforms::text::map_columns;

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
        match &self.steps[previous] {
            Step::EditCells(edits) => {
                edits.apply(&self.read_level(previous, range.clone())?, range.start)
            }
            Step::InsertRows { at, count } => {
                let (at, count) = (*at, *count);
                let mut parts = Vec::new();
                let before = range.start..range.end.min(at);
                if !before.is_empty() {
                    parts.push(self.read_level(previous, before)?);
                }
                let inserted = range.start.max(at)..range.end.min(at + count);
                if !inserted.is_empty() {
                    parts.push(empty_rows(&schema, inserted.len())?);
                }
                let after = range.start.max(at + count)..range.end;
                if !after.is_empty() {
                    parts.push(self.read_level(previous, after.start - count..after.end - count)?);
                }
                concat(&schema, parts)
            }
            Step::DeleteRows { rows } => {
                let parts = kept_ranges(rows, range)
                    .into_iter()
                    .map(|r| self.read_level(previous, r))
                    .collect::<Result<Vec<_>>>()?;
                concat(&schema, parts)
            }
            Step::AddColumn { name, .. } => {
                let batch = self.read_level(previous, range)?;
                let index = schema.index_of(name)?;
                let mut columns = batch.columns().to_vec();
                columns.insert(
                    index,
                    new_null_array(schema.field(index).data_type(), batch.num_rows()),
                );
                with_schema(schema, columns, batch.num_rows())
            }
            // Full-pass steps are read through their cache above.
            Step::Cast(cast) => cast.apply(&self.read_level(previous, range)?, &schema),
            Step::Split(split) => split.apply(&self.read_level(previous, range)?, &schema),
            Step::CustomColumn(custom) => custom.apply(
                &self.read_level(previous, range.clone())?,
                range.start,
                &schema,
            ),
            Step::Merge(merge) => merge.apply(&self.read_level(previous, range)?, &schema),
            Step::AddField { column, .. } | Step::RemoveField { column, .. } => {
                crate::transforms::nested::change_field(
                    &self.read_level(previous, range)?,
                    column,
                    &schema,
                )
            }
            Step::CombineStruct { columns, name } => crate::transforms::nested::combine(
                &self.read_level(previous, range)?,
                columns,
                name,
                &schema,
            ),
            Step::Replace(replace) => map_columns(
                &self.read_level(previous, range)?,
                &schema,
                std::slice::from_ref(&replace.column),
                |c| replace.apply(c),
            ),
            Step::FormatText { columns, op } => {
                map_columns(&self.read_level(previous, range)?, &schema, columns, |c| {
                    op.apply(c)
                })
            }
            Step::ExpandStruct { column, all_levels } => {
                let input = self.read_level(previous, range)?;
                crate::transforms::nested::expand_levels(&input, column, *all_levels, &schema)
            }
            Step::Filter(_)
            | Step::Sort(_)
            | Step::Fill(_)
            | Step::RemoveEmptyColumn { .. }
            | Step::ExplodeList { .. }
            | Step::CollectList { .. } => Err(invalid!(ComputeFirst)),
            Step::RemoveColumns { .. }
            | Step::KeepColumns { .. }
            | Step::RenameColumn { .. }
            | Step::MoveColumn { .. } => {
                // Pick the previous columns by name in the new order; renames
                // map the new name back to the old one.
                let batch = self.read_level(previous, range)?;
                let prev_schema = &self.levels[previous].schema;
                let columns = schema
                    .fields()
                    .iter()
                    .map(|f| {
                        let name = match &self.steps[previous] {
                            Step::RenameColumn { from, to } if f.name() == to => from,
                            _ => f.name(),
                        };
                        Ok(batch.column(prev_schema.index_of(name)?).clone())
                    })
                    .collect::<Result<Vec<_>>>()?;
                with_schema(schema, columns, batch.num_rows())
            }
        }
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
        let full = &self.levels[level].schema;
        let schema = project_schema(full, names);
        let previous = level - 1;
        let prev_schema = &self.levels[previous].schema;
        let read = |names: &[String], range| self.read_level_columns(previous, range, names);
        // The columns of `batch` (read from the previous level) for `schema`,
        // each found under `source(name)`.
        let pick = |batch: &RecordBatch, source: &dyn Fn(&String) -> String| {
            let input = batch.schema();
            let columns = schema
                .fields()
                .iter()
                .map(|f| Ok(batch.column(input.index_of(&source(f.name()))?).clone()))
                .collect::<Result<Vec<_>>>()?;
            with_schema(schema.clone(), columns, batch.num_rows())
        };
        let same = |name: &String| name.clone();
        if let Some(cache) = &self.caches[previous]
            && !matches!(cache.as_ref(), Cache::Checked(_))
        {
            return match cache.as_ref() {
                Cache::Rows(rows) => {
                    let parts = read_kept(&rows.map(range), |span| read(names, span))?;
                    concat(&schema, parts)
                }
                Cache::Data(source) => source.read_columns(range, names),
                Cache::Project(_) => pick(&read(names, range)?, &same),
                Cache::Checked(_) => unreachable!("handled above"),
            };
        }
        match &self.steps[previous] {
            Step::EditCells(edits) => {
                let batch = edits.apply(&read(names, range.clone())?, range.start)?;
                pick(&batch, &same)
            }
            Step::InsertRows { at, count } => {
                let (at, count) = (*at, *count);
                let mut parts = Vec::new();
                let before = range.start..range.end.min(at);
                if !before.is_empty() {
                    parts.push(read(names, before)?);
                }
                let inserted = range.start.max(at)..range.end.min(at + count);
                if !inserted.is_empty() {
                    parts.push(empty_rows(&schema, inserted.len())?);
                }
                let after = range.start.max(at + count)..range.end;
                if !after.is_empty() {
                    parts.push(read(names, after.start - count..after.end - count)?);
                }
                concat(&schema, parts)
            }
            Step::DeleteRows { rows } => {
                let parts = kept_ranges(rows, range)
                    .into_iter()
                    .map(|r| read(names, r))
                    .collect::<Result<Vec<_>>>()?;
                concat(&schema, parts)
            }
            Step::AddColumn { name, .. } => {
                let others: Vec<String> = names.iter().filter(|n| *n != name).cloned().collect();
                let batch = read(&others, range)?;
                let rows = batch.num_rows();
                let input = batch.schema();
                let columns = schema
                    .fields()
                    .iter()
                    .map(|f| match f.name() == name {
                        true => Ok(new_null_array(f.data_type(), rows)),
                        false => Ok(batch.column(input.index_of(f.name())?).clone()),
                    })
                    .collect::<Result<Vec<_>>>()?;
                with_schema(schema, columns, rows)
            }
            Step::RemoveColumns { .. }
            | Step::KeepColumns { .. }
            | Step::RenameColumn { .. }
            | Step::MoveColumn { .. } => {
                let source = |name: &String| match &self.steps[previous] {
                    Step::RenameColumn { from, to } if name == to => from.clone(),
                    _ => name.clone(),
                };
                let wanted: Vec<String> =
                    schema.fields().iter().map(|f| source(f.name())).collect();
                pick(&read(&wanted, range)?, &source)
            }
            Step::Filter(_)
            | Step::Sort(_)
            | Step::Fill(_)
            | Step::RemoveEmptyColumn { .. }
            | Step::ExplodeList { .. }
            | Step::CollectList { .. } => Err(invalid!(ComputeFirst)),
            step => {
                let computed = computed_columns(step, prev_schema, full);
                let passes = schema.fields().iter().all(|f| {
                    !computed.contains(f.name()) && prev_schema.index_of(f.name()).is_ok()
                });
                match passes {
                    true => pick(&read(names, range)?, &same),
                    false => crate::source::project(&self.read_level(level, range)?, names),
                }
            }
        }
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
fn project_schema(schema: &SchemaRef, names: &[String]) -> SchemaRef {
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

/// Output columns a column step computes or changes: new names, and the
/// columns it changes in place. Every other column passes through it as is.
fn computed_columns(
    step: &Step,
    input: &SchemaRef,
    output: &SchemaRef,
) -> std::collections::HashSet<String> {
    let mut computed: std::collections::HashSet<String> = output
        .fields()
        .iter()
        .filter(|f| input.index_of(f.name()).is_err())
        .map(|f| f.name().clone())
        .collect();
    let changed: Vec<&String> = match step {
        Step::Cast(cast) => vec![&cast.column],
        Step::Replace(replace) => vec![&replace.column],
        Step::FormatText { columns, .. } => columns.iter().collect(),
        Step::AddField { column, .. } | Step::RemoveField { column, .. } => vec![column],
        _ => Vec::new(),
    };
    computed.extend(changed.into_iter().cloned());
    computed
}

/// A batch of `rows` rows with every value null.
fn empty_rows(schema: &SchemaRef, rows: usize) -> Result<RecordBatch> {
    let columns = schema
        .fields()
        .iter()
        .map(|f| new_null_array(f.data_type(), rows))
        .collect();
    with_schema(schema.clone(), columns, rows)
}

pub(super) fn with_schema(
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
