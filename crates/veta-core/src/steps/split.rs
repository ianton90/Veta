//! Splitting a column into several, and merging columns into one.

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, StringArray, StringBuilder};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use super::invalid;
use crate::error::Result;

/// Splits a column's text into `parts` new text columns named
/// `<column>.1`, `<column>.2`, …, which replace it.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub column: String,
    pub by: SplitBy,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SplitBy {
    /// At each occurrence of `delimiter`, into at most `parts` columns; the
    /// last one keeps the rest of the text.
    Delimiter { delimiter: String, parts: usize },
    /// At these character positions (increasing), into `positions + 1`
    /// columns.
    Positions(Vec<usize>),
}

impl Split {
    pub fn describe(&self) -> String {
        match &self.by {
            SplitBy::Delimiter { delimiter, parts } => {
                format!(
                    "Split {} by {delimiter:?} into {parts} columns",
                    self.column
                )
            }
            SplitBy::Positions(positions) => {
                let list: Vec<String> = positions.iter().map(ToString::to_string).collect();
                format!("Split {} at positions {}", self.column, list.join(", "))
            }
        }
    }

    fn parts(&self) -> usize {
        match &self.by {
            SplitBy::Delimiter { parts, .. } => *parts,
            SplitBy::Positions(positions) => positions.len() + 1,
        }
    }

    pub fn part_names(&self) -> Vec<String> {
        (1..=self.parts())
            .map(|i| format!("{}.{i}", self.column))
            .collect()
    }

    pub(super) fn output_fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        let index = column_index(schema, &self.column)?;
        match &self.by {
            SplitBy::Delimiter { delimiter, parts } => {
                if delimiter.is_empty() {
                    return Err(invalid("enter a delimiter".into()));
                }
                if *parts < 2 {
                    return Err(invalid("split into at least 2 columns".into()));
                }
            }
            SplitBy::Positions(positions) => {
                if positions.is_empty() {
                    return Err(invalid("enter at least one position".into()));
                }
                if positions[0] == 0 || positions.windows(2).any(|w| w[0] >= w[1]) {
                    return Err(invalid("positions must be increasing and above 0".into()));
                }
            }
        }
        let names = self.part_names();
        let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
        for name in &names {
            if fields
                .iter()
                .enumerate()
                .any(|(i, f)| i != index && f.name() == name)
            {
                return Err(invalid(format!("a column named {name:?} already exists")));
            }
        }
        fields.splice(
            index..=index,
            names.iter().map(|n| Field::new(n, DataType::Utf8, true)),
        );
        Ok(fields)
    }

    pub(super) fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        let index = column_index(&batch.schema(), &self.column)?;
        let strings = cast(batch.column(index), &DataType::Utf8)?;
        let strings = as_strings(&strings)?;
        let parts = self.parts();
        let mut builders: Vec<StringBuilder> = (0..parts).map(|_| StringBuilder::new()).collect();
        let mut pieces: Vec<&str> = Vec::with_capacity(parts);
        for value in strings.iter() {
            pieces.clear();
            if let Some(s) = value {
                match &self.by {
                    SplitBy::Delimiter { delimiter, .. } => {
                        pieces.extend(s.splitn(parts, delimiter.as_str()));
                    }
                    SplitBy::Positions(positions) => {
                        let mut start = 0;
                        for &p in positions {
                            let end = byte_offset(s, p).max(start);
                            pieces.push(&s[start..end]);
                            start = end;
                        }
                        pieces.push(&s[start..]);
                    }
                }
            }
            for (i, builder) in builders.iter_mut().enumerate() {
                builder.append_option(pieces.get(i).copied());
            }
        }
        let mut columns = batch.columns().to_vec();
        columns.splice(
            index..=index,
            builders
                .into_iter()
                .map(|mut b| Arc::new(b.finish()) as ArrayRef),
        );
        Ok(RecordBatch::try_new(schema.clone(), columns)?)
    }
}

/// Joins columns' text (in the listed order) with a separator into a new
/// column `name`, placed where the leftmost of them was; the merged columns
/// are removed. Nulls count as empty text; all-null rows stay null.
#[derive(Debug, Clone, PartialEq)]
pub struct Merge {
    pub columns: Vec<String>,
    pub separator: String,
    pub name: String,
}

impl Merge {
    pub fn describe(&self) -> String {
        format!("Merged {} into {}", self.columns.join(", "), self.name)
    }

    pub(super) fn output_fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        if self.columns.len() < 2 {
            return Err(invalid("select at least 2 columns to merge".into()));
        }
        let mut indices = Vec::new();
        for name in &self.columns {
            let i = column_index(schema, name)?;
            if indices.contains(&i) {
                return Err(invalid(format!("{name} is listed twice")));
            }
            indices.push(i);
        }
        if self.name.trim().is_empty() {
            return Err(invalid("column name cannot be empty".into()));
        }
        let first = indices.iter().copied().min().unwrap_or_default();
        let mut fields = Vec::new();
        for (i, field) in schema.fields().iter().enumerate() {
            if i == first {
                fields.push(Field::new(&self.name, DataType::Utf8, true));
            } else if !indices.contains(&i) {
                if field.name() == &self.name {
                    return Err(invalid(format!(
                        "a column named {:?} already exists",
                        self.name
                    )));
                }
                fields.push(field.as_ref().clone());
            }
        }
        Ok(fields)
    }

    pub(super) fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        let input = batch.schema();
        let indices = self
            .columns
            .iter()
            .map(|c| column_index(&input, c))
            .collect::<Result<Vec<_>>>()?;
        let texts = indices
            .iter()
            .map(|&i| cast(batch.column(i), &DataType::Utf8))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let texts = texts.iter().map(as_strings).collect::<Result<Vec<_>>>()?;
        let mut merged = StringBuilder::new();
        let mut line = String::new();
        for row in 0..batch.num_rows() {
            if texts.iter().all(|t| t.is_null(row)) {
                merged.append_null();
                continue;
            }
            line.clear();
            for (k, t) in texts.iter().enumerate() {
                if k > 0 {
                    line.push_str(&self.separator);
                }
                if t.is_valid(row) {
                    line.push_str(t.value(row));
                }
            }
            merged.append_value(&line);
        }
        let merged: ArrayRef = Arc::new(merged.finish());
        let first = indices.iter().copied().min().unwrap_or_default();
        let mut columns = Vec::new();
        for (i, column) in batch.columns().iter().enumerate() {
            if i == first {
                columns.push(merged.clone());
            } else if !indices.contains(&i) {
                columns.push(column.clone());
            }
        }
        Ok(RecordBatch::try_new(schema.clone(), columns)?)
    }
}

fn column_index(schema: &Schema, name: &str) -> Result<usize> {
    schema
        .index_of(name)
        .map_err(|_| invalid(format!("no column named {name:?}")))
}

fn as_strings(array: &ArrayRef) -> Result<&StringArray> {
    array
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| invalid("expected text".into()))
}

/// Byte offset of character `chars` in `s` (the end if past it).
fn byte_offset(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i)
}

/// Most parts any value in `sample` splits into by `delimiter`.
pub fn count_parts(sample: &ArrayRef, delimiter: &str) -> usize {
    if delimiter.is_empty() {
        return 1;
    }
    let Ok(strings) = cast(sample, &DataType::Utf8) else {
        return 1;
    };
    let Ok(strings) = as_strings(&strings) else {
        return 1;
    };
    strings
        .iter()
        .flatten()
        .map(|s| s.matches(delimiter).count() + 1)
        .max()
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int64Array;

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("full", DataType::Utf8, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec![Some("a,b,c,d"), Some("ñu"), None])),
            ],
        )
        .unwrap()
    }

    fn strings(batch: &RecordBatch, column: &str) -> Vec<Option<String>> {
        as_strings(batch.column_by_name(column).unwrap())
            .unwrap()
            .iter()
            .map(|s| s.map(str::to_owned))
            .collect()
    }

    fn run(split: &Split) -> RecordBatch {
        let input = batch();
        let schema = Arc::new(Schema::new(split.output_fields(&input.schema()).unwrap()));
        split.apply(&input, &schema).unwrap()
    }

    #[test]
    fn split_by_delimiter_and_position() {
        let split = Split {
            column: "full".into(),
            by: SplitBy::Delimiter {
                delimiter: ",".into(),
                parts: 3,
            },
        };
        let out = run(&split);
        assert_eq!(out.num_columns(), 4);
        assert_eq!(out.schema().field(1).name(), "full.1");
        assert_eq!(strings(&out, "full.3")[0].as_deref(), Some("c,d"));
        assert_eq!(strings(&out, "full.2")[1], None);
        assert_eq!(strings(&out, "full.1")[2], None);

        let split = Split {
            column: "full".into(),
            by: SplitBy::Positions(vec![1, 3]),
        };
        let out = run(&split);
        assert_eq!(strings(&out, "full.1")[1].as_deref(), Some("ñ"));
        assert_eq!(strings(&out, "full.2")[1].as_deref(), Some("u"));
        assert_eq!(strings(&out, "full.3")[1].as_deref(), Some(""));
        assert_eq!(strings(&out, "full.2")[0].as_deref(), Some(",b"));

        let bad = Split {
            column: "full".into(),
            by: SplitBy::Positions(vec![3, 2]),
        };
        assert!(bad.output_fields(&batch().schema()).is_err());
        assert_eq!(count_parts(batch().column(1), ","), 4);
    }

    #[test]
    fn merge_columns() {
        let merge = Merge {
            columns: vec!["full".into(), "id".into()],
            separator: "-".into(),
            name: "key".into(),
        };
        let input = batch();
        let schema = Arc::new(Schema::new(merge.output_fields(&input.schema()).unwrap()));
        let out = merge.apply(&input, &schema).unwrap();
        // Placed where "id" (the first of them in the table) was.
        assert_eq!(out.num_columns(), 1);
        assert_eq!(
            strings(&out, "key"),
            [
                Some("a,b,c,d-1".into()),
                Some("ñu-2".into()),
                Some("-3".into())
            ]
        );
        let clash = Merge {
            columns: vec!["full".into(), "full".into()],
            separator: String::new(),
            name: "x".into(),
        };
        assert!(clash.output_fields(&input.schema()).is_err());
    }
}
