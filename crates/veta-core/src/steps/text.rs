//! Row-by-row value transforms: replacing values and formatting text.

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, Scalar, StringArray};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::zip::zip;
use arrow::compute::{cast, is_null, prep_null_mask_filter};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;

use super::invalid;
use crate::error::Result;
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
    pub fn describe(&self) -> String {
        let find = match &self.find {
            Some(f) => format!("{f:?}"),
            None => "nulls".into(),
        };
        let with = match &self.with {
            Some(w) => format!("{w:?}"),
            None => "null".into(),
        };
        format!("Replaced {find} with {with} in {}", self.column)
    }

    fn substring(&self, data_type: &DataType) -> bool {
        is_text(data_type) && !self.whole_cell && self.find.is_some()
    }

    pub(super) fn validate(&self, schema: &Schema) -> Result<()> {
        let field = schema
            .field_with_name(&self.column)
            .map_err(|_| invalid(format!("no column named {:?}", self.column)))?;
        let data_type = field.data_type();
        if self.find.is_none() && self.with.is_none() {
            return Err(invalid(
                "replacing nulls needs a value to put instead".into(),
            ));
        }
        if self.substring(data_type) {
            if self.find.as_deref() == Some("") {
                return Err(invalid("enter the text to find".into()));
            }
            return Ok(());
        }
        if let Some(find) = &self.find
            && !is_text(data_type)
            && find.trim().is_empty()
        {
            return Err(invalid("enter the value to find".into()));
        }
        parse_value(self.find.as_deref(), data_type)?;
        parse_value(self.with.as_deref(), data_type)?;
        Ok(())
    }

    pub(super) fn apply(&self, column: &ArrayRef) -> Result<ArrayRef> {
        let data_type = column.data_type();
        if self.substring(data_type) {
            let find = self.find.as_deref().unwrap_or_default();
            let with = self.with.as_deref().unwrap_or_default();
            return map_text(column, |s| s.replace(find, with));
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

    pub fn verb(self) -> &'static str {
        match self {
            Self::Trim => "Trimmed",
            Self::Lower => "Lowercased",
            Self::Upper => "Uppercased",
            Self::Title => "Capitalized",
        }
    }

    pub(super) fn validate(self, schema: &Schema, columns: &[String]) -> Result<()> {
        if columns.is_empty() {
            return Err(invalid("no columns to format".into()));
        }
        for name in columns {
            let field = schema
                .field_with_name(name)
                .map_err(|_| invalid(format!("no column named {name:?}")))?;
            if !is_text(field.data_type()) {
                return Err(invalid(format!("{name} is not a text column")));
            }
        }
        Ok(())
    }

    pub(super) fn apply(self, column: &ArrayRef) -> Result<ArrayRef> {
        match self {
            Self::Trim => map_text(column, |s| s.trim().to_owned()),
            Self::Lower => map_text(column, str::to_lowercase),
            Self::Upper => map_text(column, str::to_uppercase),
            Self::Title => map_text(column, title_case),
        }
    }
}

impl std::fmt::Display for TextOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Trim => "Trim spaces",
            Self::Lower => "lowercase",
            Self::Upper => "UPPERCASE",
            Self::Title => "Capitalize Each Word",
        })
    }
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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
    out
}

/// Nulls in a comparison mask count as false.
fn not_null(mask: BooleanArray) -> BooleanArray {
    if mask.nulls().is_some() {
        prep_null_mask_filter(&mask)
    } else {
        mask
    }
}

/// Applies `f` to each text value, keeping the column's type.
fn map_text(column: &ArrayRef, f: impl Fn(&str) -> String) -> Result<ArrayRef> {
    let strings = cast(column, &DataType::Utf8)?;
    let strings = strings
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| invalid("expected text".into()))?;
    let mapped: ArrayRef = Arc::new(strings.iter().map(|s| s.map(&f)).collect::<StringArray>());
    Ok(cast(&mapped, column.data_type())?)
}

/// Replaces the named columns of `batch` using `f`; `schema` is the output
/// schema.
pub(super) fn map_columns(
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
            .map_err(|_| invalid(format!("no column named {name:?}")))?;
        columns[index] = f(&columns[index])?;
    }
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
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
