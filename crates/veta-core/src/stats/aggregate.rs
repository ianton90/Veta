//! [`Aggregate`]: the statistics of one column, in one pass, computing only
//! what the statistics asked for need. Pieces of the column are aggregated
//! on several threads and merged in order.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;

use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::{DataType, Float64Type};
use arrow::row::{RowConverter, SortField};

use super::accumulator::{as_numbers, extremes};
use super::distinct::{Identity, for_each_hash};
use super::full::{PARTS, count_parts, into_parts};
use super::sample::quantile_unsorted;
use crate::error::{Result, invalid};

/// A statistic of a column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stat {
    /// Rows, nulls included.
    Len,
    /// Values that aren't null.
    CountValues,
    CountNulls,
    /// Distinct values, nulls left out.
    NUnique,
    Min,
    Max,
    /// Numbers only.
    Sum,
    Mean,
    Median,
    /// Sample standard deviation (divides by n − 1).
    Std,
    /// Sample variance (divides by n − 1).
    Var,
    /// The `q` quantile (0 to 1), interpolating linearly.
    Quantile(f64),
    /// The most frequent value (the first to appear, of equals).
    Mode,
    /// Sample skewness (adjusted Fisher–Pearson, as pandas).
    Skew,
    /// Sample excess kurtosis (as pandas).
    Kurtosis,
}

/// How a quantile between two values is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// Between them, in proportion.
    #[default]
    Linear,
    /// The lower one.
    Lower,
    /// The higher one.
    Higher,
    /// The nearer one (the even position, halfway).
    Nearest,
    /// Halfway between them.
    Midpoint,
}

/// The `q` quantile (0 to 1) of `values`, in any order (reordered, not
/// sorted).
pub(crate) fn quantile(values: &mut [f64], q: f64, how: Interpolation) -> Option<f64> {
    if how == Interpolation::Linear {
        return quantile_unsorted(values, q);
    }
    let last = values.len().checked_sub(1)?;
    let at = q.clamp(0.0, 1.0) * last as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    let (_, &mut below, rest) = values.select_nth_unstable_by(low, f64::total_cmp);
    let above = match high > low {
        true => rest.iter().copied().fold(f64::INFINITY, f64::min),
        false => below,
    };
    Some(match how {
        Interpolation::Lower | Interpolation::Linear => below,
        Interpolation::Higher => above,
        Interpolation::Midpoint => (below + above) / 2.0,
        Interpolation::Nearest => match (at - low as f64).total_cmp(&0.5) {
            std::cmp::Ordering::Less => below,
            std::cmp::Ordering::Greater => above,
            std::cmp::Ordering::Equal if low % 2 == 0 => below,
            std::cmp::Ordering::Equal => above,
        },
    })
}

/// What the statistics asked for need from each value.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Needs {
    extremes: bool,
    /// Moments up to this order (0: none, 2: mean and spread, 4: skew and
    /// kurtosis).
    moments: u8,
    sum: bool,
    values: bool,
    distinct: bool,
    counts: bool,
}

impl Needs {
    pub(crate) fn of(stats: &[Stat]) -> Self {
        let mut needs = Self::default();
        for stat in stats {
            match stat {
                Stat::Len | Stat::CountValues | Stat::CountNulls => {}
                Stat::NUnique => needs.distinct = true,
                Stat::Min | Stat::Max => needs.extremes = true,
                Stat::Sum => needs.sum = true,
                Stat::Mean | Stat::Std | Stat::Var => needs.moments = needs.moments.max(2),
                Stat::Skew | Stat::Kurtosis => needs.moments = 4,
                Stat::Median | Stat::Quantile(_) => needs.values = true,
                Stat::Mode => needs.counts = true,
            }
        }
        needs
    }
}

/// A statistic's value.
#[derive(Debug, Clone)]
pub enum StatValue {
    /// A count.
    Count(u64),
    /// A number; `None` when there are no values to compute it from.
    Number(Option<f64>),
    /// A value of the column's type (an array of one value, null when there
    /// are no values).
    Value(ArrayRef),
}

impl StatValue {
    pub fn as_count(&self) -> Option<u64> {
        match self {
            StatValue::Count(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            StatValue::Number(x) => *x,
            StatValue::Count(n) => Some(*n as f64),
            StatValue::Value(_) => None,
        }
    }
}

/// Distinct hashes: a piece's set, or the pieces' hashes split into parts
/// (a hash may be in several pieces) and counted at the end.
#[derive(Debug)]
enum Distinct {
    Set(HashSet<u64, BuildHasherDefault<Identity>>),
    Parts(Vec<Vec<u64>>),
    Counted(u64),
}

/// One column's running statistics.
#[derive(Debug)]
pub(crate) struct Aggregate {
    needs: Needs,
    data_type: DataType,
    rows: u64,
    nulls: u64,
    min: Option<ArrayRef>,
    max: Option<ArrayRef>,
    /// Finite numbers seen, their mean and sums of powers of deviations
    /// from it (2nd to 4th).
    n: u64,
    mean: f64,
    m2: f64,
    m3: f64,
    m4: f64,
    /// Compensated sum (Neumaier).
    sum: f64,
    carry: f64,
    values: Vec<f64>,
    converter: Option<RowConverter>,
    distinct: Option<Distinct>,
    /// For the mode: each value's count and first row, by hash.
    counts: Option<HashMap<u64, (u64, u64), BuildHasherDefault<Identity>>>,
}

impl Aggregate {
    /// For statistics with `needs` of a column of `data_type`; an error if
    /// one doesn't apply to it.
    pub(crate) fn new(needs: Needs, data_type: &DataType, column: &str) -> Result<Self> {
        let numeric = as_numbers(&arrow::array::new_empty_array(data_type))?.is_some();
        if (needs.moments > 0 || needs.values) && !numeric {
            return Err(invalid!(NotNumericColumn, column = column));
        }
        if needs.sum && !(numeric && !data_type.is_temporal()) {
            return Err(invalid!(NotNumericColumn, column = column));
        }
        let hashed = needs.distinct || needs.counts;
        let converter = match hashed {
            true => Some(
                RowConverter::new(vec![SortField::new(data_type.clone())])
                    .map_err(|_| invalid!(UniqueUnsupported))?,
            ),
            false => None,
        };
        Ok(Self {
            needs,
            data_type: data_type.clone(),
            rows: 0,
            nulls: 0,
            min: None,
            max: None,
            n: 0,
            mean: 0.0,
            m2: 0.0,
            m3: 0.0,
            m4: 0.0,
            sum: 0.0,
            carry: 0.0,
            values: Vec::new(),
            converter,
            distinct: needs.distinct.then(|| Distinct::Set(HashSet::default())),
            counts: needs.counts.then(HashMap::default),
        })
    }

    /// Adds `array`, the column's values from row `first_row` on.
    pub(crate) fn add(&mut self, array: &ArrayRef, first_row: u64) -> Result<()> {
        self.rows += array.len() as u64;
        self.nulls += array.logical_null_count() as u64;
        if self.needs.extremes {
            extremes(array, &mut self.min, &mut self.max)?;
        }
        if let Some(converter) = &self.converter {
            let (distinct, counts) = (&mut self.distinct, &mut self.counts);
            for_each_hash(array, first_row, converter, |hash, n, row| {
                if let Some(Distinct::Set(set)) = distinct {
                    set.insert(hash);
                }
                if let Some(counts) = counts {
                    let entry = counts.entry(hash).or_insert((0, row));
                    entry.0 += n;
                    entry.1 = entry.1.min(row);
                }
            })?;
        }
        let numbers = self.needs.moments > 0 || self.needs.sum || self.needs.values;
        if numbers && let Some(floats) = as_numbers(array)? {
            let floats = floats.as_primitive::<Float64Type>();
            let finite: Vec<f64> = floats.iter().flatten().filter(|x| x.is_finite()).collect();
            self.add_numbers(&finite);
        }
        Ok(())
    }

    fn add_numbers(&mut self, xs: &[f64]) {
        if self.needs.sum {
            for &x in xs {
                self.add_to_sum(x);
            }
        }
        if self.needs.values {
            self.values.extend_from_slice(xs);
        }
        if self.needs.moments > 0 && !xs.is_empty() {
            // The batch's moments in two passes, then combined.
            let n = xs.len() as f64;
            let mean = xs.iter().map(|x| x / n).sum::<f64>();
            let (mut m2, mut m3, mut m4) = (0.0, 0.0, 0.0);
            for &x in xs {
                let d = x - mean;
                m2 += d * d;
                if self.needs.moments > 2 {
                    m3 += d * d * d;
                    m4 += d * d * d * d;
                }
            }
            self.combine(xs.len() as u64, mean, m2, m3, m4);
        }
    }

    /// Adds `n` numbers with mean `mean` and sums of powers of deviations
    /// `m2`, `m3`, `m4` (Pébay's formulas).
    fn combine(&mut self, n: u64, mean: f64, m2: f64, m3: f64, m4: f64) {
        if n == 0 {
            return;
        }
        let (na, nb) = (self.n as f64, n as f64);
        let total = na + nb;
        let delta = mean - self.mean;
        let (d2, d3, d4) = (
            delta * delta,
            delta * delta * delta,
            delta * delta * delta * delta,
        );
        let m4 = self.m4
            + m4
            + d4 * na * nb * (na * na - na * nb + nb * nb) / (total * total * total)
            + 6.0 * d2 * (na * na * m2 + nb * nb * self.m2) / (total * total)
            + 4.0 * delta * (na * m3 - nb * self.m3) / total;
        let m3 = self.m3
            + m3
            + d3 * na * nb * (na - nb) / (total * total)
            + 3.0 * delta * (na * m2 - nb * self.m2) / total;
        let m2 = self.m2 + m2 + d2 * na * nb / total;
        // Weighted, so means near the largest float don't overflow.
        self.mean = self.mean * (na / total) + mean * (nb / total);
        (self.n, self.m2, self.m3, self.m4) = (self.n + n, m2, m3, m4);
    }

    /// Adds what `other` (the same column, the rows after) counted.
    pub(crate) fn merge(&mut self, other: Aggregate) {
        self.rows += other.rows;
        self.nulls += other.nulls;
        if self.needs.extremes {
            for value in [other.min, other.max].into_iter().flatten() {
                // Both come from arrays of one value of the same type.
                let _ = extremes(&value, &mut self.min, &mut self.max);
            }
        }
        if self.needs.sum {
            self.add_to_sum(other.sum);
            self.carry += other.carry;
        }
        self.values.extend(other.values);
        self.combine(other.n, other.mean, other.m2, other.m3, other.m4);
        if let Some(Distinct::Set(mine)) = &mut self.distinct {
            let mut parts = vec![Vec::new(); PARTS];
            into_parts(&mut parts, std::mem::take(mine));
            self.distinct = Some(Distinct::Parts(parts));
        }
        match (&mut self.distinct, other.distinct) {
            (Some(Distinct::Parts(parts)), Some(Distinct::Set(theirs))) => {
                into_parts(parts, theirs)
            }
            (Some(Distinct::Parts(parts)), Some(Distinct::Parts(theirs))) => {
                for (mine, theirs) in parts.iter_mut().zip(theirs) {
                    mine.extend(theirs);
                }
            }
            _ => {}
        }
        if let (Some(mine), Some(theirs)) = (&mut self.counts, other.counts) {
            for (hash, (n, row)) in theirs {
                let entry = mine.entry(hash).or_insert((0, row));
                entry.0 += n;
                entry.1 = entry.1.min(row);
            }
        }
    }

    fn add_to_sum(&mut self, x: f64) {
        let total = self.sum + x;
        self.carry += match self.sum.abs() >= x.abs() {
            true => (self.sum - total) + x,
            false => (x - total) + self.sum,
        };
        self.sum = total;
    }

    /// The variance dividing by n − `ddof`.
    pub(crate) fn variance(&self, ddof: u32) -> Option<f64> {
        let divisor = self.n as f64 - ddof as f64;
        (divisor > 0.0)
            .then(|| self.m2 / divisor)
            .filter(|v| v.is_finite())
    }

    /// The `q` quantile, taken as `how` says.
    pub(crate) fn quantile(&mut self, q: f64, how: Interpolation) -> Option<f64> {
        quantile(&mut self.values, q, how)
    }

    /// The row of the mode (the most frequent value, the first of equals).
    pub(crate) fn mode_row(&self) -> Option<u64> {
        let counts = self.counts.as_ref()?;
        counts
            .values()
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|&(_, row)| row)
    }

    /// The value of `stat`; the mode as its row (read by the caller).
    pub(crate) fn finish(&mut self, stat: Stat) -> StatValue {
        let null = || arrow::array::new_null_array(&self.data_type, 1);
        let n = self.n as f64;
        let number = |x: f64| StatValue::Number(x.is_finite().then_some(x));
        match stat {
            Stat::Len => StatValue::Count(self.rows),
            Stat::CountValues => StatValue::Count(self.rows - self.nulls),
            Stat::CountNulls => StatValue::Count(self.nulls),
            Stat::NUnique => {
                let count = match self.distinct.take() {
                    Some(Distinct::Set(set)) => set.len() as u64,
                    Some(Distinct::Parts(parts)) => count_parts(parts),
                    Some(Distinct::Counted(count)) => count,
                    None => 0,
                };
                self.distinct = Some(Distinct::Counted(count));
                StatValue::Count(count)
            }
            Stat::Min => StatValue::Value(self.min.clone().unwrap_or_else(null)),
            Stat::Max => StatValue::Value(self.max.clone().unwrap_or_else(null)),
            Stat::Sum => StatValue::Number(Some(self.sum + self.carry)),
            Stat::Mean => StatValue::Number((self.n > 0).then_some(self.mean)),
            Stat::Var => StatValue::Number(self.variance(1)),
            Stat::Std => StatValue::Number(self.variance(1).map(f64::sqrt)),
            Stat::Median => StatValue::Number(quantile_unsorted(&mut self.values, 0.5)),
            Stat::Quantile(q) => StatValue::Number(quantile_unsorted(&mut self.values, q)),
            Stat::Skew if self.n > 2 && self.m2 > 0.0 => {
                let g1 = n.sqrt() * self.m3 / self.m2.powf(1.5);
                number(g1 * (n * (n - 1.0)).sqrt() / (n - 2.0))
            }
            Stat::Kurtosis if self.n > 3 && self.m2 > 0.0 => {
                let g2 = n * self.m4 / (self.m2 * self.m2) - 3.0;
                number(((n + 1.0) * g2 + 6.0) * (n - 1.0) / ((n - 2.0) * (n - 3.0)))
            }
            Stat::Skew | Stat::Kurtosis => StatValue::Number(None),
            Stat::Mode => StatValue::Number(self.mode_row().map(|r| r as f64)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow::array::Float64Array;

    fn numbers(xs: &[f64]) -> ArrayRef {
        Arc::new(Float64Array::from(xs.to_vec()))
    }

    #[test]
    fn merged_pieces_match_one_piece() {
        let xs: Vec<f64> = (0..1000).map(|i| ((i * 37) % 101) as f64 / 3.0).collect();
        let stats = [
            Stat::Mean,
            Stat::Var,
            Stat::Skew,
            Stat::Kurtosis,
            Stat::Sum,
            Stat::Median,
        ];
        let needs = Needs::of(&stats);
        let mut whole = Aggregate::new(needs, &DataType::Float64, "x").unwrap();
        whole.add(&numbers(&xs), 0).unwrap();
        let mut pieces = Aggregate::new(needs, &DataType::Float64, "x").unwrap();
        for (i, chunk) in xs.chunks(77).enumerate() {
            let mut piece = Aggregate::new(needs, &DataType::Float64, "x").unwrap();
            piece.add(&numbers(chunk), (i * 77) as u64).unwrap();
            pieces.merge(piece);
        }
        for stat in stats {
            let (a, b) = (
                whole.finish(stat).as_number().unwrap(),
                pieces.finish(stat).as_number().unwrap(),
            );
            assert!(
                (a - b).abs() < 1e-9 * a.abs().max(1.0),
                "{stat:?}: {a} vs {b}"
            );
        }
    }

    #[test]
    fn known_values() {
        // pandas: skew 0.0, kurt -1.2 for 1..=5; var 2.5.
        let mut agg = Aggregate::new(
            Needs::of(&[Stat::Var, Stat::Skew, Stat::Kurtosis]),
            &DataType::Float64,
            "x",
        )
        .unwrap();
        agg.add(&numbers(&[1.0, 2.0, 3.0, 4.0, 5.0]), 0).unwrap();
        assert_eq!(agg.finish(Stat::Var).as_number(), Some(2.5));
        assert!(agg.finish(Stat::Skew).as_number().unwrap().abs() < 1e-12);
        assert!((agg.finish(Stat::Kurtosis).as_number().unwrap() + 1.2).abs() < 1e-12);
        assert!(Aggregate::new(Needs::of(&[Stat::Mean]), &DataType::Utf8, "t").is_err());
    }
}
