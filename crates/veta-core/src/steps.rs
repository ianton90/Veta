//! Applied steps and the engine that evaluates them.
//!
//! A document's data is its source followed by a list of [`Step`]s, each
//! transforming the output of the previous one (Power Query's "Applied
//! Steps"). [`Pipeline`] evaluates the steps lazily over a row window, so only
//! the rows on screen (or the batch being saved) are ever computed.
//!
//! Steps refer to columns by name and to rows by their index in the previous
//! step's output.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, new_null_array};
use arrow::compute::{concat_batches, interleave};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use crate::display::type_name;
use crate::error::{Error, Result};
use crate::source::DataSource;

/// One transformation of the data.
#[derive(Debug, Clone)]
pub enum Step {
    /// Replaces individual cell values.
    EditCells(CellEdits),
    /// Inserts `count` empty rows before row `at`.
    InsertRows {
        at: usize,
        count: usize,
    },
    /// Deletes rows. Ranges are sorted, disjoint and non-empty.
    DeleteRows {
        rows: Vec<Range<usize>>,
    },
    /// Adds an empty column at position `at`.
    AddColumn {
        name: String,
        data_type: DataType,
        at: usize,
    },
    RemoveColumns {
        names: Vec<String>,
    },
    RenameColumn {
        from: String,
        to: String,
    },
    /// Moves a column to position `to`.
    MoveColumn {
        name: String,
        to: usize,
    },
}

impl Step {
    /// Short description for the applied steps list.
    pub fn describe(&self) -> String {
        fn plural(n: usize, one: &str) -> String {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {one}s")
            }
        }
        match self {
            Step::EditCells(edits) => format!("Edited {}", plural(edits.len(), "cell")),
            Step::InsertRows { count, .. } => format!("Inserted {}", plural(*count, "row")),
            Step::DeleteRows { rows } => {
                let n = rows.iter().map(|r| r.len()).sum();
                format!("Deleted {}", plural(n, "row"))
            }
            Step::AddColumn {
                name, data_type, ..
            } => {
                format!("Added column {name} ({})", type_name(data_type))
            }
            Step::RemoveColumns { names } => match names.as_slice() {
                [one] => format!("Removed column {one}"),
                _ => format!("Removed {} columns", names.len()),
            },
            Step::RenameColumn { from, to } => format!("Renamed {from} to {to}"),
            Step::MoveColumn { name, .. } => format!("Moved column {name}"),
        }
    }
}

/// Edited cell values: column name → row → new value (a one-element array of
/// the column's type, possibly null).
#[derive(Debug, Clone, Default)]
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

    fn validate(&self, level: &Level) -> Result<()> {
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
    fn apply(&self, batch: &RecordBatch, start: usize) -> Result<RecordBatch> {
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

/// Shape of the data after a step.
#[derive(Debug, Clone)]
struct Level {
    schema: SchemaRef,
    num_rows: usize,
}

impl Level {
    fn index(&self, column: &str) -> Result<usize> {
        self.schema
            .index_of(column)
            .map_err(|_| invalid(format!("no column named {column:?}")))
    }

    fn field(&self, column: &str) -> Result<&Field> {
        Ok(self.schema.field(self.index(column)?))
    }

    /// Validates `step` against this level and returns the level after it.
    fn next(&self, step: &Step) -> Result<Level> {
        let fields: Vec<Field> = self
            .schema
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect();
        let with_fields = |fields: Vec<Field>, num_rows: usize| Level {
            schema: Arc::new(Schema::new_with_metadata(
                fields,
                self.schema.metadata().clone(),
            )),
            num_rows,
        };
        match step {
            Step::EditCells(edits) => {
                edits.validate(self)?;
                Ok(self.clone())
            }
            Step::InsertRows { at, count } => {
                if *at > self.num_rows {
                    return Err(invalid(format!(
                        "cannot insert rows after row {}",
                        self.num_rows
                    )));
                }
                if *count == 0 {
                    return Err(invalid("nothing to insert".into()));
                }
                // Inserted rows are empty, so every column must allow nulls.
                let fields = fields.into_iter().map(|f| f.with_nullable(true)).collect();
                Ok(with_fields(fields, self.num_rows + count))
            }
            Step::DeleteRows { rows } => {
                if rows.is_empty() {
                    return Err(invalid("no rows to delete".into()));
                }
                let mut previous_end = 0;
                for (i, r) in rows.iter().enumerate() {
                    if r.is_empty() || (i > 0 && r.start <= previous_end) {
                        return Err(invalid("row ranges must be sorted and disjoint".into()));
                    }
                    if r.end > self.num_rows {
                        return Err(invalid(format!(
                            "row {} is past the last row ({})",
                            r.end, self.num_rows
                        )));
                    }
                    previous_end = r.end;
                }
                let deleted: usize = rows.iter().map(|r| r.len()).sum();
                Ok(Level {
                    schema: self.schema.clone(),
                    num_rows: self.num_rows - deleted,
                })
            }
            Step::AddColumn {
                name,
                data_type,
                at,
            } => {
                check_new_name(&self.schema, name)?;
                if *at > fields.len() {
                    return Err(invalid(format!("column position {at} is out of range")));
                }
                let mut fields = fields;
                fields.insert(*at, Field::new(name, data_type.clone(), true));
                Ok(with_fields(fields, self.num_rows))
            }
            Step::RemoveColumns { names } => {
                if names.is_empty() {
                    return Err(invalid("no columns to remove".into()));
                }
                for name in names {
                    self.index(name)?;
                }
                let fields: Vec<Field> = fields
                    .into_iter()
                    .filter(|f| !names.contains(f.name()))
                    .collect();
                if fields.is_empty() {
                    return Err(invalid("at least one column must remain".into()));
                }
                Ok(with_fields(fields, self.num_rows))
            }
            Step::RenameColumn { from, to } => {
                let index = self.index(from)?;
                if from != to {
                    check_new_name(&self.schema, to)?;
                }
                let mut fields = fields;
                fields[index] = fields[index].clone().with_name(to);
                Ok(with_fields(fields, self.num_rows))
            }
            Step::MoveColumn { name, to } => {
                let index = self.index(name)?;
                if *to >= fields.len() {
                    return Err(invalid(format!("column position {to} is out of range")));
                }
                let mut fields = fields;
                let field = fields.remove(index);
                fields.insert(*to, field);
                Ok(with_fields(fields, self.num_rows))
            }
        }
    }
}

fn check_new_name(schema: &Schema, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(invalid("column name cannot be empty".into()));
    }
    if schema.fields().iter().any(|f| f.name() == name) {
        return Err(invalid(format!("a column named {name:?} already exists")));
    }
    Ok(())
}

fn invalid(message: String) -> Error {
    Error::InvalidCommand(message)
}

/// A source plus its steps, evaluated lazily.
#[derive(Debug, Clone)]
pub struct Pipeline {
    source: Arc<dyn DataSource>,
    steps: Vec<Step>,
    /// `levels[0]` is the source; `levels[i + 1]` is the output of `steps[i]`.
    levels: Vec<Level>,
}

impl Pipeline {
    pub fn new(source: Arc<dyn DataSource>) -> Self {
        let level = Level {
            schema: source.schema(),
            num_rows: source.num_rows(),
        };
        Self {
            source,
            steps: Vec::new(),
            levels: vec![level],
        }
    }

    pub fn source(&self) -> &Arc<dyn DataSource> {
        &self.source
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn schema(&self) -> SchemaRef {
        self.last().schema.clone()
    }

    pub fn num_rows(&self) -> usize {
        self.last().num_rows
    }

    fn last(&self) -> &Level {
        // There is always at least the source level.
        &self.levels[self.levels.len() - 1]
    }

    /// Appends a step after validating it against the current output.
    pub fn push(&mut self, step: Step) -> Result<()> {
        let level = self.last().next(&step)?;
        self.steps.push(step);
        self.levels.push(level);
        Ok(())
    }

    /// Removes and returns the last step.
    pub fn pop(&mut self) -> Option<Step> {
        let step = self.steps.pop()?;
        self.levels.pop();
        Some(step)
    }

    /// Replaces the last step after validating the replacement. Returns the
    /// step it replaced.
    pub fn replace_last(&mut self, step: Step) -> Result<Step> {
        let Some(previous) = self.levels.len().checked_sub(2).map(|i| &self.levels[i]) else {
            return Err(invalid("there is no step to replace".into()));
        };
        let level = previous.next(&step)?;
        let n = self.levels.len();
        self.levels[n - 1] = level;
        let last = self.steps.len() - 1;
        Ok(std::mem::replace(&mut self.steps[last], step))
    }

    /// Reads the output rows in `range` (clamped to the row count).
    pub fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        let end = range.end.min(self.num_rows());
        self.read_level(self.steps.len(), range.start.min(end)..end)
    }

    /// Reads rows of level `level`; `range` must be within it.
    fn read_level(&self, level: usize, range: Range<usize>) -> Result<RecordBatch> {
        if level == 0 {
            return self.source.read(range);
        }
        let schema = self.levels[level].schema.clone();
        let previous = level - 1;
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
            Step::RemoveColumns { .. } | Step::RenameColumn { .. } | Step::MoveColumn { .. } => {
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

fn with_schema(schema: SchemaRef, columns: Vec<ArrayRef>, rows: usize) -> Result<RecordBatch> {
    let options = RecordBatchOptions::new().with_row_count(Some(rows));
    Ok(RecordBatch::try_new_with_options(
        schema, columns, &options,
    )?)
}

/// Concatenates parts, giving them `schema` (parts may differ only in
/// nullability).
fn concat(schema: &SchemaRef, parts: Vec<RecordBatch>) -> Result<RecordBatch> {
    let parts = parts
        .into_iter()
        .map(|b| {
            let rows = b.num_rows();
            with_schema(schema.clone(), b.columns().to_vec(), rows)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(concat_batches(schema, &parts)?)
}

/// Maps an output row range of a delete step to the previous step's row
/// ranges that survive.
fn kept_ranges(deleted: &[Range<usize>], output: Range<usize>) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    // Walk kept segments of the previous level, tracking their output offset.
    let mut prev_start = 0; // start of the current kept segment (previous level)
    let mut out_start = 0; // its position in the output
    let push = |segment: Range<usize>, out_offset: usize, result: &mut Vec<Range<usize>>| {
        let seg_out = out_offset..out_offset + segment.len();
        let from = output.start.max(seg_out.start);
        let to = output.end.min(seg_out.end);
        if from < to {
            let shift = segment.start as isize - seg_out.start as isize;
            result.push((from as isize + shift) as usize..(to as isize + shift) as usize);
        }
    };
    for d in deleted {
        let segment = prev_start..d.start;
        if out_start >= output.end {
            return result;
        }
        push(segment.clone(), out_start, &mut result);
        out_start += segment.len();
        prev_start = d.end;
    }
    if out_start < output.end {
        // Last kept segment runs to the end of the previous level.
        push(
            prev_start..prev_start + (output.end - out_start),
            out_start,
            &mut result,
        );
    }
    result
}

/// Sorts and merges row ranges, dropping empty ones.
pub fn normalize_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|r| !r.is_empty());
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Row ranges, not a range of values.
mod tests {
    use super::*;
    use crate::source::MemorySource;
    use arrow::array::{Int64Array, StringArray};

    fn source(rows: i64) -> Arc<dyn DataSource> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values(0..rows)),
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|i| format!("n{i}")),
                )),
            ],
        )
        .unwrap();
        Arc::new(MemorySource::new(schema, vec![batch]).unwrap())
    }

    fn ids(batch: &RecordBatch) -> Vec<Option<i64>> {
        let column = batch.column_by_name("id").unwrap();
        let column = column.as_any().downcast_ref::<Int64Array>().unwrap();
        column.iter().collect()
    }

    fn names(batch: &RecordBatch) -> Vec<String> {
        batch
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    fn value(v: i64) -> ArrayRef {
        Arc::new(Int64Array::from(vec![v]))
    }

    #[test]
    fn edit_cells_within_a_window() {
        let mut p = Pipeline::new(source(10));
        let mut edits = CellEdits::default();
        edits.set("id", 2, value(200));
        edits.set("id", 8, value(800));
        p.push(Step::EditCells(edits)).unwrap();

        let all: Vec<_> = ids(&p.read(0..10).unwrap()).into_iter().flatten().collect();
        assert_eq!(all, [0, 1, 200, 3, 4, 5, 6, 7, 800, 9]);
        assert_eq!(ids(&p.read(7..9).unwrap()), [Some(7), Some(800)]);
        assert_eq!(ids(&p.read(3..5).unwrap()), [Some(3), Some(4)]);
    }

    #[test]
    fn edits_are_validated() {
        let mut p = Pipeline::new(source(3));
        let mut edits = CellEdits::default();
        edits.set("id", 5, value(1));
        assert!(p.push(Step::EditCells(edits)).is_err());

        let mut edits = CellEdits::default();
        edits.set("id", 0, new_null_array(&DataType::Int64, 1));
        assert!(p.push(Step::EditCells(edits)).is_err(), "id is required");

        let mut edits = CellEdits::default();
        edits.set("name", 0, value(1));
        assert!(p.push(Step::EditCells(edits)).is_err(), "wrong type");
        assert!(p.steps().is_empty());
    }

    #[test]
    fn insert_rows() {
        let mut p = Pipeline::new(source(5));
        p.push(Step::InsertRows { at: 2, count: 3 }).unwrap();
        assert_eq!(p.num_rows(), 8);
        assert!(p.schema().field(0).is_nullable());
        assert_eq!(
            ids(&p.read(0..8).unwrap()),
            [
                Some(0),
                Some(1),
                None,
                None,
                None,
                Some(2),
                Some(3),
                Some(4)
            ]
        );
        assert_eq!(ids(&p.read(4..6).unwrap()), [None, Some(2)]);
        p.push(Step::InsertRows { at: 8, count: 1 }).unwrap();
        assert_eq!(ids(&p.read(7..9).unwrap()), [Some(4), None]);
        assert!(p.push(Step::InsertRows { at: 100, count: 1 }).is_err());
    }

    #[test]
    fn delete_rows() {
        let mut p = Pipeline::new(source(10));
        p.push(Step::DeleteRows {
            rows: vec![1..3, 5..6, 9..10],
        })
        .unwrap();
        assert_eq!(p.num_rows(), 6);
        let all: Vec<_> = ids(&p.read(0..6).unwrap()).into_iter().flatten().collect();
        assert_eq!(all, [0, 3, 4, 6, 7, 8]);
        let middle: Vec<_> = ids(&p.read(2..5).unwrap()).into_iter().flatten().collect();
        assert_eq!(middle, [4, 6, 7]);
        assert!(p.push(Step::DeleteRows { rows: vec![0..10] }).is_err());
    }

    #[test]
    fn column_steps() {
        let mut p = Pipeline::new(source(3));
        p.push(Step::AddColumn {
            name: "extra".into(),
            data_type: DataType::Float64,
            at: 1,
        })
        .unwrap();
        assert_eq!(names(&p.read(0..3).unwrap()), ["id", "extra", "name"]);
        assert_eq!(p.read(0..3).unwrap().column(1).null_count(), 3);

        p.push(Step::RenameColumn {
            from: "name".into(),
            to: "label".into(),
        })
        .unwrap();
        p.push(Step::MoveColumn {
            name: "label".into(),
            to: 0,
        })
        .unwrap();
        let batch = p.read(1..2).unwrap();
        assert_eq!(names(&batch), ["label", "id", "extra"]);
        let label = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(label.value(0), "n1");

        p.push(Step::RemoveColumns {
            names: vec!["extra".into(), "id".into()],
        })
        .unwrap();
        assert_eq!(names(&p.read(0..1).unwrap()), ["label"]);
        assert!(
            p.push(Step::RemoveColumns {
                names: vec!["label".into()]
            })
            .is_err()
        );
    }

    #[test]
    fn column_names_are_validated() {
        let mut p = Pipeline::new(source(1));
        let rename = |to: &str| Step::RenameColumn {
            from: "id".into(),
            to: to.into(),
        };
        assert!(p.push(rename("name")).is_err());
        assert!(p.push(rename(" ")).is_err());
        assert!(
            p.push(Step::RenameColumn {
                from: "missing".into(),
                to: "x".into()
            })
            .is_err()
        );
        assert!(
            p.push(Step::AddColumn {
                name: "id".into(),
                data_type: DataType::Int8,
                at: 0
            })
            .is_err()
        );
    }

    #[test]
    fn steps_compose() {
        // Delete, then insert, then edit an inserted row.
        let mut p = Pipeline::new(source(6));
        p.push(Step::DeleteRows { rows: vec![0..2] }).unwrap();
        p.push(Step::InsertRows { at: 1, count: 1 }).unwrap();
        let mut edits = CellEdits::default();
        edits.set("id", 1, value(99));
        p.push(Step::EditCells(edits)).unwrap();
        let all: Vec<_> = ids(&p.read(0..5).unwrap()).into_iter().flatten().collect();
        assert_eq!(all, [2, 99, 3, 4, 5]);

        assert!(matches!(p.pop(), Some(Step::EditCells(_))));
        assert_eq!(ids(&p.read(1..2).unwrap()), [None]);
    }

    #[test]
    fn replace_last_validates() {
        let mut p = Pipeline::new(source(3));
        p.push(Step::InsertRows { at: 0, count: 1 }).unwrap();
        assert!(
            p.replace_last(Step::InsertRows { at: 50, count: 1 })
                .is_err()
        );
        p.replace_last(Step::InsertRows { at: 3, count: 2 })
            .unwrap();
        assert_eq!(p.num_rows(), 5);
    }

    #[test]
    fn kept_ranges_math() {
        let deleted = vec![2..4, 6..7];
        // previous: 0 1 [2 3] 4 5 [6] 7 8 9 → output 0 1 4 5 7 8 9
        assert_eq!(kept_ranges(&deleted, 0..7), vec![0..2, 4..6, 7..10]);
        assert_eq!(kept_ranges(&deleted, 1..3), vec![1..2, 4..5]);
        assert_eq!(kept_ranges(&deleted, 4..5), vec![7..8]);
        assert_eq!(kept_ranges(&[0..3], 0..2), vec![3..5]);
    }

    #[test]
    fn normalize() {
        assert_eq!(
            normalize_ranges(vec![5..6, 0..2, 1..3, 7..7, 6..8]),
            vec![0..3, 5..8]
        );
    }
}
