//! Row-by-row value transforms: replacing values and formatting text.

use crate::error::invalid;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, Scalar, StringArray};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::zip::zip;
use arrow::compute::{cast, is_null, prep_null_mask_filter};
use arrow::datatypes::{DataType, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;

use crate::error::Result;
use crate::steps::{Level, Transform};
use crate::values::{is_text, parse_value};

/// Replaces values in a column (Power Query's "Replace Values").
#[derive(Debug, Clone, PartialEq)]
pub struct Replace {
    pub column: String,
    /// Value to find; `None` finds nulls.
    pub find: Option<String>,
    /// Replacement; `None` sets null.
    pub with: Option<String>,
    /// Text columns: replace only whole cells equal to `find`, instead of
    /// every occurrence inside cells. Other columns always match whole cells.
    pub whole_cell: bool,
}

impl Replace {
    fn substring(&self, data_type: &DataType) -> bool {
        is_text(data_type) && !self.whole_cell && self.find.is_some()
    }

    pub(crate) fn validate(&self, schema: &Schema) -> Result<()> {
        let field = schema
            .field_with_name(&self.column)
            .map_err(|_| invalid!(ColumnNotFound, column = self.column))?;
        let data_type = field.data_type();
        if self.find.is_none() && self.with.is_none() {
            return Err(invalid!(ReplaceNullsNeedValue));
        }
        if self.substring(data_type) {
            if self.find.as_deref() == Some("") {
                return Err(invalid!(ReplaceNoText));
            }
            return Ok(());
        }
        if let Some(find) = &self.find
            && !is_text(data_type)
            && find.trim().is_empty()
        {
            return Err(invalid!(ReplaceNoValue));
        }
        parse_value(self.find.as_deref(), data_type)?;
        parse_value(self.with.as_deref(), data_type)?;
        Ok(())
    }

    pub(crate) fn apply(&self, column: &ArrayRef) -> Result<ArrayRef> {
        let data_type = column.data_type();
        if self.substring(data_type) {
            let find = self.find.as_deref().unwrap_or_default();
            let with = self.with.as_deref().unwrap_or_default();
            return map_text(column, |s, out| {
                if find.is_empty() {
                    out.push_str(s);
                    return;
                }
                let mut last = 0;
                for (i, _) in s.match_indices(find) {
                    out.push_str(&s[last..i]);
                    out.push_str(with);
                    last = i + find.len();
                }
                out.push_str(&s[last..]);
            });
        }
        let mask = match &self.find {
            None => is_null(column)?,
            Some(find) => {
                let value = parse_value(Some(find), data_type)?;
                let mask = cmp::eq(column, &Scalar::new(value))?;
                not_null(mask)
            }
        };
        let with = parse_value(self.with.as_deref(), data_type)?;
        Ok(zip(&mask, &Scalar::new(with), column)?)
    }
}

/// A text formatting transform (Power Query's "Format").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOp {
    Trim,
    Lower,
    Upper,
    /// Capitalizes each word.
    Title,
}

impl TextOp {
    pub const ALL: [TextOp; 4] = [Self::Trim, Self::Lower, Self::Upper, Self::Title];

    pub(crate) fn validate(self, schema: &Schema, columns: &[String]) -> Result<()> {
        if columns.is_empty() {
            return Err(invalid!(NoColumnsToFormat));
        }
        for name in columns {
            let field = schema
                .field_with_name(name)
                .map_err(|_| invalid!(ColumnNotFound, column = name))?;
            if !is_text(field.data_type()) {
                return Err(invalid!(NotTextColumn, column = name));
            }
        }
        Ok(())
    }

    pub(crate) fn apply(self, column: &ArrayRef) -> Result<ArrayRef> {
        match self {
            Self::Trim => map_text(column, |s, out| out.push_str(s.trim())),
            Self::Lower => map_text(column, |s, out| {
                out.extend(s.chars().flat_map(char::to_lowercase))
            }),
            Self::Upper => map_text(column, |s, out| {
                out.extend(s.chars().flat_map(char::to_uppercase))
            }),
            Self::Title => map_text(column, title_case),
        }
    }
}

fn title_case(s: &str, out: &mut String) {
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
}

/// Nulls in a comparison mask count as false.
fn not_null(mask: BooleanArray) -> BooleanArray {
    if mask.nulls().is_some() {
        prep_null_mask_filter(&mask)
    } else {
        mask
    }
}

/// Applies `f` to each text value (writing the result into an empty
/// buffer, reused from value to value), keeping the column's type.
fn map_text(column: &ArrayRef, f: impl Fn(&str, &mut String)) -> Result<ArrayRef> {
    let strings = cast(column, &DataType::Utf8)?;
    let strings = strings
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| invalid!(ExpectedText))?;
    let mut builder =
        arrow::array::StringBuilder::with_capacity(strings.len(), strings.value_data().len());
    let mut buffer = String::new();
    for value in strings.iter() {
        match value {
            Some(s) => {
                buffer.clear();
                f(s, &mut buffer);
                builder.append_value(&buffer);
            }
            None => builder.append_null(),
        }
    }
    let mapped: ArrayRef = Arc::new(builder.finish());
    Ok(cast(&mapped, column.data_type())?)
}

/// Replaces the named columns of `batch` using `f`; `schema` is the output
/// schema.
pub(crate) fn map_columns(
    batch: &RecordBatch,
    schema: &Arc<Schema>,
    names: &[String],
    f: impl Fn(&ArrayRef) -> Result<ArrayRef>,
) -> Result<RecordBatch> {
    let mut columns = batch.columns().to_vec();
    for name in names {
        let index = batch
            .schema()
            .index_of(name)
            .map_err(|_| invalid!(ColumnNotFound, column = name))?;
        columns[index] = f(&columns[index])?;
    }
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
}

impl Transform for Replace {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (column != self.column).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        self.validate(&input.schema)?;
        Ok(input.clone())
    }

    fn apply(&self, batch: &RecordBatch, _: usize, output: &SchemaRef) -> Result<RecordBatch> {
        let columns = std::slice::from_ref(&self.column);
        map_columns(batch, output, columns, |c| Replace::apply(self, c))
    }

    fn changed_columns(&self) -> Vec<&str> {
        vec![&self.column]
    }
}

/// [`Step::FormatText`](crate::steps::Step::FormatText).
pub(crate) struct FormatText<'a> {
    pub(crate) columns: &'a [String],
    pub(crate) op: TextOp,
}

impl Transform for FormatText<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (!self.columns.iter().any(|c| c == column)).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        self.op.validate(&input.schema, self.columns)?;
        Ok(input.clone())
    }

    fn apply(&self, batch: &RecordBatch, _: usize, output: &SchemaRef) -> Result<RecordBatch> {
        map_columns(batch, output, self.columns, |c| self.op.apply(c))
    }

    fn changed_columns(&self) -> Vec<&str> {
        self.columns.iter().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int64Array;

    fn text(values: &[Option<&str>]) -> ArrayRef {
        Arc::new(StringArray::from(values.to_vec()))
    }

    fn strings(array: &ArrayRef) -> Vec<Option<String>> {
        let array = array.as_any().downcast_ref::<StringArray>().unwrap();
        array.iter().map(|s| s.map(str::to_owned)).collect()
    }

    fn replace(find: Option<&str>, with: Option<&str>, whole_cell: bool) -> Replace {
        Replace {
            column: "c".into(),
            find: find.map(str::to_owned),
            with: with.map(str::to_owned),
            whole_cell,
        }
    }

    #[test]
    fn replace_text_values() {
        let column = text(&[Some("aba"), Some("a"), None, Some("b")]);
        let out = replace(Some("a"), Some("x"), false).apply(&column).unwrap();
        assert_eq!(
            strings(&out),
            [Some("xbx".into()), Some("x".into()), None, Some("b".into())]
        );
        let out = replace(Some("a"), None, true).apply(&column).unwrap();
        assert_eq!(
            strings(&out),
            [Some("aba".into()), None, None, Some("b".into())]
        );
        let out = replace(None, Some("?"), false).apply(&column).unwrap();
        assert_eq!(strings(&out)[2].as_deref(), Some("?"));
    }

    #[test]
    fn replace_numbers_and_validation() {
        let column: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), None, Some(2), Some(1)]));
        let out = replace(Some("1"), Some("10"), false)
            .apply(&column)
            .unwrap();
        let out = out.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(
            out.iter().collect::<Vec<_>>(),
            [Some(10), None, Some(2), Some(10)]
        );

        let schema = Schema::new(vec![arrow::datatypes::Field::new(
            "c",
            DataType::Int64,
            true,
        )]);
        assert!(
            replace(Some("x"), Some("1"), true)
                .validate(&schema)
                .is_err()
        );
        assert!(replace(None, None, true).validate(&schema).is_err());
        assert!(replace(None, Some("0"), true).validate(&schema).is_ok());
        assert!(
            replace(Some(""), Some("0"), true)
                .validate(&schema)
                .is_err()
        );
    }

    #[test]
    fn text_ops() {
        let column = text(&[Some("  hello wORLD-x "), None]);
        let run = |op: TextOp| strings(&op.apply(&column).unwrap())[0].clone().unwrap();
        assert_eq!(run(TextOp::Trim), "hello wORLD-x");
        assert_eq!(run(TextOp::Upper), "  HELLO WORLD-X ");
        assert_eq!(run(TextOp::Lower), "  hello world-x ");
        assert_eq!(run(TextOp::Title), "  Hello World-X ");
        let schema = Schema::new(vec![arrow::datatypes::Field::new(
            "n",
            DataType::Int64,
            true,
        )]);
        assert!(TextOp::Trim.validate(&schema, &["n".into()]).is_err());
    }
}
