//! Keeping rows that match conditions (Power Query's "Keep Rows" / column
//! filters).

use crate::error::invalid;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, BooleanArray, Scalar, StringArray};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::comparison::{contains, ends_with, ilike, starts_with};
use arrow::compute::{and_kleene, cast, not, or_kleene, prep_null_mask_filter};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;

use crate::display::type_name;
use crate::error::Error;
use crate::error::Result;
use crate::steps::{Cache, ComputeJob, Input, Level, RowMap, Transform, compute_first};
use crate::values::{is_text, parse_value};
use std::ops::Range;

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
            // Ignoring case, the text tests and (in)equality match with
            // Arrow's `ilike` on the strings as they are; only ordering
            // comparisons lowercase every value.
            let text_match = matches!(
                self.op,
                FilterOp::Equals
                    | FilterOp::NotEquals
                    | FilterOp::Contains
                    | FilterOp::NotContains
                    | FilterOp::StartsWith
                    | FilterOp::EndsWith
            );
            if !self.case_sensitive && text_match {
                let escaped = escape_like(&self.value);
                let pattern = match self.op {
                    FilterOp::Contains | FilterOp::NotContains => format!("%{escaped}%"),
                    FilterOp::StartsWith => format!("{escaped}%"),
                    FilterOp::EndsWith => format!("%{escaped}"),
                    _ => escaped,
                };
                let pattern = Scalar::new(StringArray::from(vec![pattern]));
                let matched = ilike(&strings, &pattern)?;
                return Ok(match self.op {
                    FilterOp::NotContains | FilterOp::NotEquals => not(&matched)?,
                    _ => matched,
                });
            }
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

/// `text` as a `like` pattern matching just itself.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
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

impl Transform for Filter {
    fn streams(&self, _: &ComputeJob) -> bool {
        true
    }

    fn stream<'j>(
        &self,
        job: &'j ComputeJob,
        input_rows: usize,
    ) -> Result<Box<dyn Iterator<Item = Result<RecordBatch>> + Send + 'j>> {
        // Pieces are read and filtered on several threads.
        let filter = self.clone();
        let pieces = job.read_parallel(input_rows, None, move |batches, _| {
            batches
                .iter()
                .map(|batch| {
                    let mask = filter.evaluate(batch)?;
                    Ok(arrow::compute::filter_record_batch(batch, &mask)?)
                })
                .collect::<Result<Vec<_>>>()
        });
        Ok(Box::new(pieces.flat_map(|piece| match piece {
            Ok(batches) => batches.into_iter().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        })))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        self.validate(&input.schema)?;
        Ok(input.clone())
    }

    fn full_pass(&self) -> bool {
        true
    }

    fn read(&self, _: &Input<'_>, _: Range<usize>, _: Option<&[String]>) -> Result<RecordBatch> {
        compute_first()
    }

    fn compute(
        &self,
        job: &ComputeJob,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Cache> {
        // Only the columns the conditions test are read.
        let mut names: Vec<String> = self.conditions.iter().map(|c| c.column.clone()).collect();
        names.dedup();
        // Pieces are read and tested on several threads; the kept
        // ranges are joined in order.
        let tested = self.clone();
        let pieces = job.read_parallel(input_rows, Some(names), move |batches, first| {
            let mut kept = Vec::new();
            let mut start = first;
            for batch in &batches {
                let mask = tested.evaluate(batch)?;
                for (from, to) in mask.values().set_slices() {
                    kept.push(start + from..start + to);
                }
                start += batch.num_rows();
            }
            Ok((kept, start))
        });
        let mut rows = RowMap::default();
        for piece in pieces {
            let (kept, end) = piece?;
            for range in kept {
                rows.push(range);
            }
            if !progress(end as f32 / input_rows as f32) {
                return Err(Error::Cancelled);
            }
        }
        Ok(Cache::Rows(rows))
    }
}

#[cfg(test)]
mod tests;
