//! Keeping rows that match conditions (Power Query's "Keep Rows" / column
//! filters).

use crate::error::invalid;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, Scalar, StringArray};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::comparison::{contains, ends_with, starts_with};
use arrow::compute::{and_kleene, cast, not, or_kleene, prep_null_mask_filter};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;

use crate::display::type_name;
use crate::error::Result;
use crate::values::{is_text, parse_value};

/// Rows are kept when all (or any) conditions hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub conditions: Vec<Condition>,
    /// `true`: every condition must hold; `false`: at least one.
    pub match_all: bool,
    /// Remove the matching rows instead of keeping them.
    pub exclude: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub column: String,
    pub op: FilterOp,
    /// Parsed as the column's type for comparisons; text for text tests.
    pub value: String,
    /// For text tests and text comparisons.
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Equals,
    NotEquals,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    IsNull,
    IsNotNull,
}

impl FilterOp {
    pub const ALL: [FilterOp; 12] = [
        Self::Equals,
        Self::NotEquals,
        Self::Less,
        Self::LessOrEqual,
        Self::Greater,
        Self::GreaterOrEqual,
        Self::Contains,
        Self::NotContains,
        Self::StartsWith,
        Self::EndsWith,
        Self::IsNull,
        Self::IsNotNull,
    ];

    pub fn is_equality(self) -> bool {
        matches!(self, Self::Equals | Self::NotEquals)
    }

    pub fn needs_value(self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }

    /// Tests on the text of the value.
    pub fn is_text_test(self) -> bool {
        matches!(
            self,
            Self::Contains | Self::NotContains | Self::StartsWith | Self::EndsWith
        )
    }
}

impl Condition {}

impl Filter {
    /// Checks the filter against the input schema.
    pub(crate) fn validate(&self, schema: &Schema) -> Result<()> {
        if self.conditions.is_empty() {
            return Err(invalid!(FilterNoConditions));
        }
        for c in &self.conditions {
            let field = schema
                .field_with_name(&c.column)
                .map_err(|_| invalid!(ColumnNotFound, column = c.column))?;
            let nested = field.data_type().is_nested();
            if nested && !c.op.is_text_test() && c.op.needs_value() && !c.op.is_equality() {
                return Err(invalid!(FilterOpNotForNested, column = c.column));
            }
            if nested && c.op.is_equality() {
                // The value is JSON, checked against the column's type.
                parse_value(Some(&c.value), field.data_type())
                    .map_err(|e| invalid!(FilterValue, column = c.column; cause = e))?;
                continue;
            }
            if c.op.needs_value() && !c.op.is_text_test() && !is_text(field.data_type()) {
                parse_value(Some(&c.value), field.data_type()).map_err(|_| {
                    invalid!(
                        FilterValueType,
                        value = c.value,
                        data_type = type_name(field.data_type()),
                        column = c.column
                    )
                })?;
                if c.value.trim().is_empty() {
                    return Err(invalid!(FilterNoValue, column = c.column));
                }
            }
        }
        Ok(())
    }

    /// Which rows of `batch` to keep (nulls count as not kept).
    pub(crate) fn evaluate(&self, batch: &RecordBatch) -> Result<BooleanArray> {
        let mut result: Option<BooleanArray> = None;
        for condition in &self.conditions {
            let mask = condition.evaluate(batch)?;
            let mask = if mask.nulls().is_some() {
                prep_null_mask_filter(&mask)
            } else {
                mask
            };
            result = Some(match result {
                None => mask,
                Some(acc) if self.match_all => and_kleene(&acc, &mask)?,
                Some(acc) => or_kleene(&acc, &mask)?,
            });
        }
        let kept = result.unwrap_or_else(|| BooleanArray::from(vec![true; batch.num_rows()]));
        Ok(if self.exclude { not(&kept)? } else { kept })
    }
}

impl Condition {
    fn evaluate(&self, batch: &RecordBatch) -> Result<BooleanArray> {
        let column = batch
            .column_by_name(&self.column)
            .ok_or_else(|| invalid!(ColumnNotFound, column = self.column))?;
        let data_type = column.data_type().clone();
        match self.op {
            FilterOp::IsNull => return Ok(arrow::compute::is_null(column)?),
            FilterOp::IsNotNull => return Ok(arrow::compute::is_not_null(column)?),
            _ => {}
        }

        // Nested values compare and match as JSON text. Equality uses the
        // value written back as JSON, so spacing doesn't matter.
        let nested = data_type.is_nested();
        if nested && self.op.is_equality() {
            let parsed = parse_value(Some(&self.value), &data_type)?;
            let value = crate::display::value_json(parsed.as_ref(), 0)?.unwrap_or_default();
            let strings: ArrayRef = Arc::new(crate::display::json_column(column.as_ref())?);
            let scalar = Scalar::new(StringArray::from(vec![value]));
            return compare(self.op, &strings, &scalar);
        }

        // Text tests, and comparisons on text columns, work on strings.
        if self.op.is_text_test() || is_text(&data_type) {
            let mut strings: ArrayRef = if nested {
                Arc::new(crate::display::json_column(column.as_ref())?)
            } else {
                cast(column, &DataType::Utf8)?
            };
            let mut value = self.value.clone();
            if !self.case_sensitive {
                strings = lowercase(&strings);
                value = value.to_lowercase();
            }
            let scalar = Scalar::new(StringArray::from(vec![value]));
            return Ok(match self.op {
                FilterOp::Contains => contains(&strings, &scalar)?,
                FilterOp::NotContains => not(&contains(&strings, &scalar)?)?,
                FilterOp::StartsWith => starts_with(&strings, &scalar)?,
                FilterOp::EndsWith => ends_with(&strings, &scalar)?,
                op => compare(op, &strings, &scalar)?,
            });
        }

        let value = parse_value(Some(&self.value), &data_type)?;
        compare(self.op, column, &Scalar::new(value))
    }
}

fn compare(op: FilterOp, left: &ArrayRef, right: &dyn arrow::array::Datum) -> Result<BooleanArray> {
    Ok(match op {
        FilterOp::Equals => cmp::eq(left, right)?,
        FilterOp::NotEquals => cmp::neq(left, right)?,
        FilterOp::Less => cmp::lt(left, right)?,
        FilterOp::LessOrEqual => cmp::lt_eq(left, right)?,
        FilterOp::Greater => cmp::gt(left, right)?,
        FilterOp::GreaterOrEqual => cmp::gt_eq(left, right)?,
        _ => return Err(invalid!(NotAComparison)),
    })
}

/// Lowercases a Utf8 array.
fn lowercase(array: &ArrayRef) -> ArrayRef {
    let Some(strings) = array.as_any().downcast_ref::<StringArray>() else {
        return array.clone();
    };
    Arc::new(
        strings
            .iter()
            .map(|s| s.map(str::to_lowercase))
            .collect::<StringArray>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float64Array, Int64Array};
    use arrow::datatypes::Field;

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("score", DataType::Float64, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
                Arc::new(StringArray::from(vec![
                    Some("Ana"),
                    Some("bob"),
                    None,
                    Some("anabel"),
                ])),
                Arc::new(Float64Array::from(vec![
                    Some(1.5),
                    None,
                    Some(3.0),
                    Some(10.0),
                ])),
            ],
        )
        .unwrap()
    }

    fn kept(filter: &Filter) -> Vec<bool> {
        filter
            .evaluate(&batch())
            .unwrap()
            .iter()
            .map(|b| b.unwrap())
            .collect()
    }

    fn condition(column: &str, op: FilterOp, value: &str) -> Condition {
        Condition {
            column: column.into(),
            op,
            value: value.into(),
            case_sensitive: false,
        }
    }

    fn one(c: Condition) -> Filter {
        Filter {
            conditions: vec![c],
            match_all: true,
            exclude: false,
        }
    }

    #[test]
    fn comparisons_skip_nulls() {
        assert_eq!(
            kept(&one(condition("score", FilterOp::Greater, "2"))),
            [false, false, true, true]
        );
        assert_eq!(
            kept(&one(condition("id", FilterOp::NotEquals, "2"))),
            [true, false, true, true]
        );
        assert_eq!(
            kept(&one(condition("score", FilterOp::IsNull, ""))),
            [false, true, false, false]
        );
    }

    #[test]
    fn text_tests_ignore_case_unless_asked() {
        assert_eq!(
            kept(&one(condition("name", FilterOp::StartsWith, "ana"))),
            [true, false, false, true]
        );
        let mut sensitive = condition("name", FilterOp::Contains, "ana");
        sensitive.case_sensitive = true;
        assert_eq!(kept(&one(sensitive)), [false, false, false, true]);
        assert_eq!(
            kept(&one(condition("name", FilterOp::Equals, "BOB"))),
            [false, true, false, false]
        );
        // Text tests work on other types through their text.
        assert_eq!(
            kept(&one(condition("score", FilterOp::Contains, ".5"))),
            [true, false, false, false]
        );
    }

    #[test]
    fn any_and_all() {
        let mut filter = Filter {
            conditions: vec![
                condition("id", FilterOp::Less, "2"),
                condition("name", FilterOp::EndsWith, "bel"),
            ],
            match_all: false,
            exclude: false,
        };
        assert_eq!(kept(&filter), [true, false, false, true]);
        filter.match_all = true;
        assert_eq!(kept(&filter), [false, false, false, false]);
        // Excluding removes the matching rows, rows with nulls included.
        filter.match_all = false;
        filter.exclude = true;
        assert_eq!(kept(&filter), [false, true, true, false]);
    }

    #[test]
    fn validation() {
        let schema = batch().schema();
        assert!(
            one(condition("score", FilterOp::Greater, "abc"))
                .validate(&schema)
                .is_err()
        );
        assert!(
            one(condition("nope", FilterOp::IsNull, ""))
                .validate(&schema)
                .is_err()
        );
        assert!(
            one(condition("score", FilterOp::Greater, ""))
                .validate(&schema)
                .is_err()
        );
        assert!(
            one(condition("name", FilterOp::Equals, ""))
                .validate(&schema)
                .is_ok()
        );
        assert!(
            Filter {
                conditions: vec![],
                match_all: true,
                exclude: false
            }
            .validate(&schema)
            .is_err()
        );
    }

    #[test]
    fn nested_columns_filter_as_json() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
        let batch = doc.read(0..4).unwrap();
        let kept = |column: &str, op: FilterOp, value: &str| -> Vec<usize> {
            let filter = Filter {
                conditions: vec![Condition {
                    column: column.into(),
                    op,
                    value: value.into(),
                    case_sensitive: false,
                }],
                match_all: true,
                exclude: false,
            };
            filter.validate(&batch.schema()).unwrap();
            let mask = filter.evaluate(&batch).unwrap();
            (0..mask.len()).filter(|&i| mask.value(i)).collect()
        };
        assert_eq!(kept("tags", FilterOp::Contains, "\"A\""), [0]);
        assert_eq!(kept("tags", FilterOp::Equals, r#"[ "c", null, "d" ]"#), [3]);
        assert_eq!(kept("point", FilterOp::Equals, r#"{"y": 2, "x": 1}"#), [0]);
        assert_eq!(
            kept("point", FilterOp::NotEquals, r#"{"x": 1, "y": 2}"#),
            [1, 3]
        );
        assert_eq!(kept("scores", FilterOp::Contains, "math"), [0, 3]);
        assert_eq!(kept("owner", FilterOp::IsNull, ""), [2]);
        let less = Filter {
            conditions: vec![Condition {
                column: "tags".into(),
                op: FilterOp::Less,
                value: "[]".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        };
        assert!(less.validate(&batch.schema()).is_err());
    }
}
