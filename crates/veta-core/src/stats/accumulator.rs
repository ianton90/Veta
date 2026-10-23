//! Running statistics of one column, added up piece by piece.

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, AsArray, Float64Array, Int32Array, Int64Array, make_comparator,
};
use arrow::compute::{SortOptions, cast};
use arrow::datatypes::{DataType, TimeUnit};
use arrow::row::{RowConverter, SortField};

use super::distinct::{Distinct, insert_hashes};
use super::sample::{Sample, quantile};
use super::{ColumnStats, Quartiles, TextLengths};
use crate::display::type_name;
use crate::error::{Error, Result, invalid};

/// Running statistics for one column.
pub(super) struct Accumulator {
    pub(super) column: String,
    data_type: DataType,
    count: u64,
    nulls: u64,
    pub(super) distinct: Option<Distinct>,
    converter: Option<RowConverter>,
    /// One-element arrays.
    min: Option<ArrayRef>,
    max: Option<ArrayRef>,
    /// Whether min and max are kept: a type with a meaningful order whose
    /// values could be compared so far.
    pub(super) orderable: bool,
    /// What mean and spread are kept for, and the running mean and sum of
    /// squared deviations (of the underlying numbers, for times).
    moments: Moments,
    n: u64,
    mean: f64,
    m2: f64,
    /// Text lengths in characters, when the column is text.
    lengths: Option<Lengths>,
    /// Sampled values, for quantiles (numbers and times).
    sample: Option<Sample>,
}

/// Whose mean and standard deviation a column gets.
#[derive(Clone, Copy, PartialEq)]
enum Moments {
    None,
    Numbers,
    /// Dates, times, timestamps and durations: from their underlying
    /// numbers, shown in the column's type (the spread as a duration).
    Temporal,
}

/// Running text length statistics.
#[derive(Clone, Copy)]
struct Lengths {
    n: u64,
    min: u64,
    max: u64,
    sum: u64,
}

impl Lengths {
    fn add(&mut self, length: u64) {
        self.min = if self.n == 0 {
            length
        } else {
            self.min.min(length)
        };
        self.max = self.max.max(length);
        self.sum += length;
        self.n += 1;
    }

    fn merge(&mut self, other: Lengths) {
        if other.n == 0 {
            return;
        }
        self.min = if self.n == 0 {
            other.min
        } else {
            self.min.min(other.min)
        };
        self.max = self.max.max(other.max);
        self.sum += other.sum;
        self.n += other.n;
    }
}

impl Accumulator {
    pub(super) fn new(column: String, data_type: DataType) -> Self {
        let converter = RowConverter::new(vec![SortField::new(data_type.clone())]).ok();
        let moments = if is_numeric(&data_type) {
            Moments::Numbers
        } else if data_type.is_temporal() {
            Moments::Temporal
        } else {
            Moments::None
        };
        let lengths = matches!(
            data_type,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
        )
        .then_some(Lengths {
            n: 0,
            min: 0,
            max: 0,
            sum: 0,
        });
        let sample = (moments != Moments::None).then(Sample::default);
        Self {
            sample,
            column,
            distinct: converter.as_ref().map(|_| Distinct::default()),
            converter,
            orderable: has_range(&data_type),
            min: None,
            max: None,
            moments,
            lengths,
            data_type,
            count: 0,
            nulls: 0,
            n: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Adds what `other` (the same column, later rows) counted.
    pub(super) fn merge(&mut self, other: Accumulator) {
        self.count += other.count;
        self.nulls += other.nulls;
        if let (Some(mine), Some(theirs)) = (&mut self.distinct, other.distinct) {
            mine.merge(theirs);
        }
        if !other.orderable {
            self.orderable = false;
        }
        if self.orderable {
            for extreme in [other.min, other.max].into_iter().flatten() {
                if self.update_extremes(&extreme).is_err() {
                    self.orderable = false;
                    break;
                }
            }
        }
        if !self.orderable {
            self.min = None;
            self.max = None;
        }
        if other.n > 0 {
            self.combine(other.n, other.mean, other.m2);
        }
        if let (Some(mine), Some(theirs)) = (&mut self.lengths, other.lengths) {
            mine.merge(theirs);
        }
        if let (Some(mine), Some(theirs)) = (&mut self.sample, other.sample) {
            mine.merge(theirs);
        }
    }

    /// Adds `n` values with mean `mean` and squared deviations `m2` to the
    /// running ones (Chan et al.'s formula).
    fn combine(&mut self, n: u64, mean: f64, m2: f64) {
        let total = self.n + n;
        let delta = mean - self.mean;
        // Weighted, so means near the largest float don't overflow.
        let (mine, theirs) = (self.n as f64 / total as f64, n as f64 / total as f64);
        self.mean = self.mean * mine + mean * theirs;
        self.m2 += m2 + delta * delta * (self.n as f64 * n as f64) / total as f64;
        self.n = total;
    }

    /// Adds `array`, the column's values from row `first_row` on.
    pub(super) fn add(&mut self, array: &ArrayRef, first_row: u64) -> Result<()> {
        self.count += array.len() as u64;
        self.nulls += array.logical_null_count() as u64;
        if let (Some(converter), Some(distinct)) = (&self.converter, &mut self.distinct) {
            insert_hashes(array, converter, distinct)?;
        }
        if self.orderable && self.update_extremes(array).is_err() {
            self.orderable = false;
            self.min = None;
            self.max = None;
        }
        if let Some(lengths) = &mut self.lengths {
            add_lengths(array, lengths)?;
        }
        if self.moments == Moments::Temporal {
            let values = cast(&underlying(array)?, &DataType::Float64)?;
            self.add_numbers(&values, first_row)?;
        }
        if self.moments == Moments::Numbers {
            let values = cast(array, &DataType::Float64)?;
            self.add_numbers(&values, first_row)?;
        }
        Ok(())
    }

    /// Adds `values` (Float64, from row `first_row` on) to the running
    /// mean and spread, and the sample.
    fn add_numbers(&mut self, values: &ArrayRef, first_row: u64) -> Result<()> {
        let values = values
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| invalid!(ExpectedNumbers))?;
        if let Some(sample) = &mut self.sample {
            for (i, x) in values.iter().enumerate() {
                if let Some(x) = x.filter(|x| x.is_finite()) {
                    sample.add(first_row + i as u64, x);
                }
            }
        }
        // The batch's mean and squared deviations in two passes, then
        // combined with the running ones.
        let finite = || values.iter().flatten().filter(|x| x.is_finite());
        let (n, sum) = finite().fold((0u64, 0.0), |(n, sum), x| (n + 1, sum + x));
        if n == 0 {
            return Ok(());
        }
        let mean = if sum.is_finite() {
            sum / n as f64
        } else {
            // Values near the largest float overflow the sum: average them
            // one at a time instead.
            finite().enumerate().fold(0.0, |mean, (i, x)| {
                let k = (i + 1) as f64;
                mean * (i as f64 / k) + x / k
            })
        };
        let m2 = finite().map(|x| (x - mean) * (x - mean)).sum();
        self.combine(n, mean, m2);
        Ok(())
    }

    pub(super) fn update_extremes(&mut self, array: &ArrayRef) -> Result<()> {
        let options = SortOptions::default();
        // Text and numbers: Arrow's kernels find the batch's extremes; other
        // types compare value by value.
        if let Some((low, high)) = batch_extremes(array) {
            let keep = |current: &Option<ArrayRef>, candidate: ArrayRef, want_less: bool| {
                let Some(current) = current else {
                    return Ok::<_, Error>(Some(candidate));
                };
                let order = make_comparator(candidate.as_ref(), current.as_ref(), options)?(0, 0);
                let better = if want_less {
                    order.is_lt()
                } else {
                    order.is_gt()
                };
                Ok(Some(if better { candidate } else { current.clone() }))
            };
            self.min = keep(&self.min, low, true)?;
            self.max = keep(&self.max, high, false)?;
            return Ok(());
        }
        let compare = make_comparator(array.as_ref(), array.as_ref(), options)?;
        let mut low: Option<usize> = None;
        let mut high: Option<usize> = None;
        for i in (0..array.len()).filter(|&i| array.is_valid(i)) {
            if low.is_none_or(|l| compare(i, l).is_lt()) {
                low = Some(i);
            }
            if high.is_none_or(|h| compare(i, h).is_gt()) {
                high = Some(i);
            }
        }
        let pick = |current: &Option<ArrayRef>, candidate: Option<usize>, want_less: bool| {
            let Some(c) = candidate else {
                return Ok(current.clone());
            };
            let candidate = array.slice(c, 1);
            let Some(current) = current else {
                return Ok(Some(candidate));
            };
            let order = make_comparator(candidate.as_ref(), current.as_ref(), options)?(0, 0);
            let better = if want_less {
                order.is_lt()
            } else {
                order.is_gt()
            };
            Ok::<_, Error>(Some(if better { candidate } else { current.clone() }))
        };
        self.min = pick(&self.min, low, true)?;
        self.max = pick(&self.max, high, false)?;
        Ok(())
    }

    pub(super) fn finish(self) -> ColumnStats {
        let (distinct, distinct_estimated) = match &self.distinct {
            Some(d) => (Some(d.count()), d.estimated()),
            None => (None, false),
        };
        let std_dev = (self.n > 1).then(|| (self.m2 / (self.n - 1) as f64).sqrt());
        let (mean, std_dev) = match self.moments {
            _ if self.n == 0 => (None, None),
            Moments::None => (None, None),
            Moments::Numbers => (Some(float_value(self.mean)), std_dev.map(float_value)),
            Moments::Temporal => (
                temporal_value(self.mean, &self.data_type).ok(),
                std_dev.and_then(|s| duration_value(s, &self.data_type).ok()),
            ),
        };
        let (sorted, sampled) = self.sample.map(Sample::finish).unwrap_or_default();
        let value = |x: f64| match self.moments {
            Moments::Temporal => temporal_value(x, &self.data_type).ok(),
            _ => Some(float_value(x)),
        };
        let quartiles = (|| {
            Some(Quartiles {
                low: value(quantile(&sorted, 0.25)?)?,
                median: value(quantile(&sorted, 0.5)?)?,
                high: value(quantile(&sorted, 0.75)?)?,
                estimated: sampled,
            })
        })();
        let text_lengths = self.lengths.filter(|l| l.n > 0).map(|l| TextLengths {
            min: l.min,
            max: l.max,
            mean: l.sum as f64 / l.n as f64,
        });
        ColumnStats {
            type_name: type_name(&self.data_type),
            count: self.count,
            nulls: self.nulls,
            distinct,
            distinct_estimated,
            min: self.min,
            max: self.max,
            mean,
            std_dev,
            text_lengths,
            quartiles,
            column: self.column,
        }
    }
}

/// Whether min and max mean something for this type: numbers, dates,
/// times and durations.
fn has_range(data_type: &DataType) -> bool {
    match data_type {
        DataType::Dictionary(_, values) => has_range(values),
        t => t.is_numeric() || t.is_temporal(),
    }
}

/// The smallest and largest valid values of `array` as one-value arrays,
/// for text and number arrays; `None` for other types (or no valid values).
fn batch_extremes(array: &ArrayRef) -> Option<(ArrayRef, ArrayRef)> {
    use arrow::array::{AsArray, StringArray};
    use arrow::compute::{max, max_string, min, min_string};
    macro_rules! primitive {
        ($t:ty) => {{
            let values = array.as_primitive::<$t>();
            let (low, high) = (min(values)?, max(values)?);
            let one = |v| -> ArrayRef {
                std::sync::Arc::new(
                    arrow::array::PrimitiveArray::<$t>::from(vec![v])
                        .with_data_type(array.data_type().clone()),
                )
            };
            Some((one(low), one(high)))
        }};
    }
    use arrow::datatypes::*;
    match array.data_type() {
        DataType::Utf8 => {
            let values = array.as_string::<i32>();
            let (low, high) = (min_string(values)?, max_string(values)?);
            Some((
                std::sync::Arc::new(StringArray::from(vec![low])),
                std::sync::Arc::new(StringArray::from(vec![high])),
            ))
        }
        DataType::Int8 => primitive!(Int8Type),
        DataType::Int16 => primitive!(Int16Type),
        DataType::Int32 => primitive!(Int32Type),
        DataType::Int64 => primitive!(Int64Type),
        DataType::UInt8 => primitive!(UInt8Type),
        DataType::UInt16 => primitive!(UInt16Type),
        DataType::UInt32 => primitive!(UInt32Type),
        DataType::UInt64 => primitive!(UInt64Type),
        DataType::Date32 => primitive!(Date32Type),
        DataType::Date64 => primitive!(Date64Type),
        DataType::Timestamp(TimeUnit::Second, _) => primitive!(TimestampSecondType),
        DataType::Timestamp(TimeUnit::Millisecond, _) => primitive!(TimestampMillisecondType),
        DataType::Timestamp(TimeUnit::Microsecond, _) => primitive!(TimestampMicrosecondType),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => primitive!(TimestampNanosecondType),
        _ => None,
    }
}

fn is_numeric(t: &DataType) -> bool {
    t.is_integer()
        || t.is_floating()
        || matches!(
            t,
            DataType::Decimal32(..)
                | DataType::Decimal64(..)
                | DataType::Decimal128(..)
                | DataType::Decimal256(..)
        )
}

/// A temporal array's underlying numbers (days, seconds, … since its
/// origin) as 64-bit integers.
fn underlying(array: &ArrayRef) -> Result<ArrayRef> {
    Ok(match cast(array, &DataType::Int64) {
        Ok(ints) => ints,
        Err(_) => cast(&cast(array, &DataType::Int32)?, &DataType::Int64)?,
    })
}

fn float_value(x: f64) -> ArrayRef {
    Arc::new(Float64Array::from(vec![x]))
}

/// The value of `data_type` whose underlying number is `x` (rounded).
fn temporal_value(x: f64, data_type: &DataType) -> Result<ArrayRef> {
    let x = x.round();
    let ints: ArrayRef = match data_type {
        DataType::Date32 | DataType::Time32(_) => Arc::new(Int32Array::from(vec![x as i32])),
        _ => Arc::new(Int64Array::from(vec![x as i64])),
    };
    Ok(cast(&ints, data_type)?)
}

/// A spread of `x` underlying units of `data_type`, as a duration.
fn duration_value(x: f64, data_type: &DataType) -> Result<ArrayRef> {
    let (unit, scale) = match data_type {
        DataType::Date32 => (TimeUnit::Second, 86_400.0),
        DataType::Date64 => (TimeUnit::Millisecond, 1.0),
        DataType::Timestamp(unit, _)
        | DataType::Time32(unit)
        | DataType::Time64(unit)
        | DataType::Duration(unit) => (*unit, 1.0),
        _ => return Err(invalid!(ExpectedNumbers)),
    };
    let ints: ArrayRef = Arc::new(Int64Array::from(vec![(x * scale).round() as i64]));
    Ok(cast(&ints, &DataType::Duration(unit))?)
}

/// Adds the length in characters of each text value of `array`.
fn add_lengths(array: &ArrayRef, lengths: &mut Lengths) -> Result<()> {
    let strings = cast(array, &DataType::Utf8)?;
    for value in strings.as_string::<i32>().iter().flatten() {
        lengths.add(value.chars().count() as u64);
    }
    Ok(())
}
