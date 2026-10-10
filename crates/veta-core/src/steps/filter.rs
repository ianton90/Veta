//! Keeping rows that match conditions (Power Query's "Keep Rows" / column
//! filters).

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, Scalar, StringArray};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::comparison::{contains, ends_with, starts_with};
use arrow::compute::{and_kleene, cast, not, or_kleene, prep_null_mask_filter};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;

use super::invalid;
use crate::display::type_name;
use crate::error::Result;
use crate::values::{is_text, parse_value};

/// Rows are kept when all (or any) conditions hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub conditions: Vec<Condition>,
    /// `true`: every condition must hold; `false`: at least one.
    pub match_all: bool,
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

impl std::fmt::Display for FilterOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Equals => "equals",
            Self::NotEquals => "does not equal",
            Self::Less => "is less than",
            Self::LessOrEqual => "is less than or equal to",
            Self::Greater => "is greater than",
            Self::GreaterOrEqual => "is greater than or equal to",
            Self::Contains => "contains",
            Self::NotContains => "does not contain",
            Self::StartsWith => "starts with",
            Self::EndsWith => "ends with",
            Self::IsNull => "is null",
            Self::IsNotNull => "is not null",
        })
    }
}

impl Condition {
    pub fn describe(&self) -> String {
        if self.op.needs_value() {
            format!("{} {} {:?}", self.column, self.op, self.value)
        } else {
            format!("{} {}", self.column, self.op)
        }
    }
}

impl Filter {
    pub fn describe(&self) -> String {
        let parts: Vec<String> = self.conditions.iter().map(Condition::describe).collect();
        let joined = parts.join(if self.match_all { " and " } else { " or " });
        format!("Kept rows where {joined}")
    }

    /// Checks the filter against the input schema.
    pub(super) fn validate(&self, schema: &Schema) -> Result<()> {
        if self.conditions.is_empty() {
            return Err(invalid("a filter needs at least one condition".into()));
        }
        for c in &self.conditions {
            let field = schema
                .field_with_name(&c.column)
                .map_err(|_| invalid(format!("no column named {:?}", c.column)))?;
            if c.op.needs_value() && !c.op.is_text_test() && !is_text(field.data_type()) {
                parse_value(Some(&c.value), field.data_type()).map_err(|_| {
                    invalid(format!(
                        "{:?} is not a valid {} for column {}",
                        c.value,
                        type_name(field.data_type()),
                        c.column
                    ))
                })?;
                if c.value.trim().is_empty() {
                    return Err(invalid(format!(
                        "enter a value to compare {} with",
                        c.column
                    )));
                }
            }
        }
        Ok(())
    }

    /// Which rows of `batch` to keep (nulls count as not kept).
    pub(super) fn evaluate(&self, batch: &RecordBatch) -> Result<BooleanArray> {
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
        Ok(result.unwrap_or_else(|| BooleanArray::from(vec![true; batch.num_rows()])))
    }
}

impl Condition {
    fn evaluate(&self, batch: &RecordBatch) -> Result<BooleanArray> {
        let column = batch
            .column_by_name(&self.column)
            .ok_or_else(|| invalid(format!("no column named {:?}", self.column)))?;
        let data_type = column.data_type().clone();
        match self.op {
            FilterOp::IsNull => return Ok(arrow::compute::is_null(column)?),
            FilterOp::IsNotNull => return Ok(arrow::compute::is_not_null(column)?),
            _ => {}
        }

        // Text tests, and comparisons on text columns, work on strings.
        if self.op.is_text_test() || is_text(&data_type) {
            let mut strings = cast(column, &DataType::Utf8)?;
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
        other => return Err(invalid(format!("{other} is not a comparison"))),
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
        };
        assert_eq!(kept(&filter), [true, false, false, true]);
        filter.match_all = true;
        assert_eq!(kept(&filter), [false, false, false, false]);
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
                match_all: true
            }
            .validate(&schema)
            .is_err()
        );
    }
}
