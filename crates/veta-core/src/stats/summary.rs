//! Per-column statistics: count, nulls, distinct, min, max, mean and
//! standard deviation, computed in one streaming pass over the document's
//! output.
//!
//! Distinct values are counted exactly (by 64-bit hash) up to
//! [`EXACT_DISTINCT_LIMIT`] values, then estimated with HyperLogLog. For a
//! file with no steps, [`quick_stats`] reads nulls, min and max from the
//! Parquet footer without touching the data.

use crate::error::invalid;
use arrow::array::{Array, ArrayRef, Float64Array, make_comparator};
use arrow::compute::{SortOptions, cast};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::row::{RowConverter, SortField};
use arrow::util::display::array_value_to_string;
use serde::Serialize;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::display::type_name;
use crate::error::{Error, Result};
use crate::steps::Pipeline;

/// Distinct values counted exactly before switching to an estimate.
pub const EXACT_DISTINCT_LIMIT: usize = 1_000_000;

/// Statistics of one column.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColumnStats {
    pub column: String,
    #[serde(rename = "type")]
    pub type_name: String,
    /// Rows looked at.
    pub count: u64,
    pub nulls: u64,
    /// Distinct non-null values; `None` if the type can't be compared.
    pub distinct: Option<u64>,
    /// Whether `distinct` is an estimate (too many values to count).
    pub distinct_estimated: bool,
    /// Smallest and largest non-null values, as one-value arrays of the
    /// column's type; only for numbers, dates, times and durations (the
    /// order of text or booleans says nothing about the data). Written as
    /// text in JSON; front ends format them for display.
    #[serde(serialize_with = "value_as_text")]
    pub min: Option<ArrayRef>,
    #[serde(serialize_with = "value_as_text")]
    pub max: Option<ArrayRef>,
    /// Numeric columns only.
    pub mean: Option<f64>,
    /// Sample standard deviation; numeric columns only.
    pub std_dev: Option<f64>,
}

/// Running statistics for one column.
struct Accumulator {
    column: String,
    data_type: DataType,
    count: u64,
    nulls: u64,
    distinct: Option<Distinct>,
    converter: Option<RowConverter>,
    /// One-element arrays.
    min: Option<ArrayRef>,
    max: Option<ArrayRef>,
    /// Whether min and max are kept: a type with a meaningful order whose
    /// values could be compared so far.
    orderable: bool,
    /// Running mean and sum of squared deviations.
    numeric: bool,
    n: u64,
    mean: f64,
    m2: f64,
}

impl Accumulator {
    fn new(column: String, data_type: DataType) -> Self {
        let converter = RowConverter::new(vec![SortField::new(data_type.clone())]).ok();
        let numeric = is_numeric(&data_type);
        Self {
            column,
            distinct: converter.as_ref().map(|_| Distinct::default()),
            converter,
            orderable: has_range(&data_type),
            min: None,
            max: None,
            numeric,
            data_type,
            count: 0,
            nulls: 0,
            n: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Adds what `other` (the same column, later rows) counted.
    fn merge(&mut self, other: Accumulator) {
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
    }

    /// Adds `n` values with mean `mean` and squared deviations `m2` to the
    /// running ones (Chan et al.'s formula).
    fn combine(&mut self, n: u64, mean: f64, m2: f64) {
        let total = self.n + n;
        let delta = mean - self.mean;
        self.mean += delta * n as f64 / total as f64;
        self.m2 += m2 + delta * delta * (self.n as f64 * n as f64) / total as f64;
        self.n = total;
    }

    fn add(&mut self, array: &ArrayRef) -> Result<()> {
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
        if self.numeric {
            let values = cast(array, &DataType::Float64)?;
            let values = values
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| invalid!(ExpectedNumbers))?;
            // The batch's mean and squared deviations in two passes, then
            // combined with the running ones.
            let finite = || values.iter().flatten().filter(|x| x.is_finite());
            let (n, sum) = finite().fold((0u64, 0.0), |(n, sum), x| (n + 1, sum + x));
            if n > 0 {
                let mean = sum / n as f64;
                let m2 = finite().map(|x| (x - mean) * (x - mean)).sum();
                self.combine(n, mean, m2);
            }
        }
        Ok(())
    }

    fn update_extremes(&mut self, array: &ArrayRef) -> Result<()> {
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

    fn finish(self) -> ColumnStats {
        let (distinct, distinct_estimated) = match &self.distinct {
            Some(d) => (Some(d.count()), d.estimated()),
            None => (None, false),
        };
        let numeric = self.numeric && self.n > 0;
        ColumnStats {
            type_name: type_name(&self.data_type),
            count: self.count,
            nulls: self.nulls,
            distinct,
            distinct_estimated,
            min: self.min,
            max: self.max,
            mean: numeric.then_some(self.mean),
            std_dev: (numeric && self.n > 1).then(|| (self.m2 / (self.n - 1) as f64).sqrt()),
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

/// Adds the hashes of `array`'s valid values to `distinct`. Numbers are
/// hashed from their value, text from its bytes, other types through the
/// row format.
fn insert_hashes(
    array: &ArrayRef,
    converter: &RowConverter,
    distinct: &mut Distinct,
) -> Result<()> {
    use arrow::array::AsArray;
    match array.data_type() {
        // Text straight from its offsets and bytes.
        DataType::Utf8 => {
            let values = array.as_string::<i32>();
            let (offsets, bytes) = (values.value_offsets(), values.value_data());
            each_valid(array, |i| {
                let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
                distinct.insert(hash_bytes(&bytes[start..end]));
            });
        }
        DataType::LargeUtf8 => {
            let values = array.as_string::<i64>();
            let (offsets, bytes) = (values.value_offsets(), values.value_data());
            each_valid(array, |i| {
                let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
                distinct.insert(hash_bytes(&bytes[start..end]));
            });
        }
        t if t.is_primitive() && t.primitive_width().is_some_and(|w| w <= 16) => {
            let width = t.primitive_width().unwrap_or(0);
            let data = array.to_data();
            let bytes = &data.buffers()[0].as_slice()[data.offset() * width..];
            // Little-endian words, zero-extended: the same value always
            // gives the same hash.
            let word = |b: &[u8]| u64::from_le_bytes(b.try_into().expect("8 bytes"));
            match width {
                8 => each_valid(array, |i| {
                    distinct.insert(mix(word(&bytes[i * 8..i * 8 + 8]) ^ SEED));
                }),
                16 => each_valid(array, |i| {
                    let (low, high) = (&bytes[i * 16..i * 16 + 8], &bytes[i * 16 + 8..i * 16 + 16]);
                    distinct.insert(mix(word(low) ^ mix(word(high) ^ SEED)));
                }),
                _ => each_valid(array, |i| {
                    distinct.insert(mix(tail_word(&bytes[i * width..(i + 1) * width]) ^ SEED));
                }),
            }
        }
        // At most two values: whether each is there.
        DataType::Boolean => {
            let values = array.as_boolean();
            let trues = values.true_count();
            let valid = values.len() - values.null_count();
            for (value, present) in [(true, trues > 0), (false, trues < valid)] {
                if present {
                    distinct.insert(mix(u64::from(value) ^ SEED));
                }
            }
        }
        _ => {
            let rows = converter.convert_columns(std::slice::from_ref(array))?;
            each_valid(array, |i| distinct.insert(hash_bytes(rows.row(i).data())));
        }
    }
    Ok(())
}

/// Calls `f` with the index of each valid value of `array`.
fn each_valid(array: &ArrayRef, mut f: impl FnMut(usize)) {
    match array.nulls().filter(|n| n.null_count() > 0) {
        Some(nulls) => nulls.valid_indices().for_each(f),
        None => (0..array.len()).for_each(&mut f),
    }
}

/// Mixed into every hash, so no value hashes to 0.
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// A fast 64-bit hash of `bytes`, well mixed in every bit (HyperLogLog uses
/// the top bits).
fn hash_bytes(bytes: &[u8]) -> u64 {
    let step = |h: u64, word: u64| {
        let h = (h ^ word).wrapping_mul(0x9fb2_1c65_1e98_df25);
        h ^ (h >> 29)
    };
    let mut h = SEED ^ bytes.len() as u64;
    let mut words = bytes.chunks_exact(8);
    for word in &mut words {
        h = step(h, u64::from_le_bytes(word.try_into().expect("8 bytes")));
    }
    let rest = words.remainder();
    if !rest.is_empty() {
        h = step(h, tail_word(rest));
    }
    mix(h)
}

/// 1–7 bytes as a little-endian word, zero-extended, read in
/// fixed-size pieces (a copy of any length would call `memcpy`).
fn tail_word(mut rest: &[u8]) -> u64 {
    let mut word = 0;
    let mut shift = 0;
    if rest.len() >= 4 {
        word = u64::from(u32::from_le_bytes(rest[..4].try_into().expect("4 bytes")));
        shift = 32;
        rest = &rest[4..];
    }
    if rest.len() >= 2 {
        word |= u64::from(u16::from_le_bytes(rest[..2].try_into().expect("2 bytes"))) << shift;
        shift += 16;
        rest = &rest[2..];
    }
    if let [last] = rest {
        word |= u64::from(*last) << shift;
    }
    word
}

/// splitmix64's finalizer.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A hasher for values that are hashes already.
#[derive(Default)]
struct Identity(u64);

impl Hasher for Identity {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = hash_bytes(bytes);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
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

/// Distinct hashes: a set while small, HyperLogLog after.
#[derive(Default)]
struct Distinct {
    exact: HashSet<u64, BuildHasherDefault<Identity>>,
    estimate: Option<HyperLogLog>,
}

impl Distinct {
    fn insert(&mut self, hash: u64) {
        match &mut self.estimate {
            Some(hll) => hll.insert(hash),
            None => {
                self.exact.insert(hash);
                if self.exact.len() > EXACT_DISTINCT_LIMIT {
                    self.start_estimating();
                }
            }
        }
    }

    /// Switches to the estimate, with the values seen so far.
    fn start_estimating(&mut self) {
        if self.estimate.is_some() {
            return;
        }
        let mut hll = HyperLogLog::new();
        for h in self.exact.drain() {
            hll.insert(h);
        }
        self.exact.shrink_to_fit();
        self.estimate = Some(hll);
    }

    /// Adds the values counted by `other`.
    fn merge(&mut self, other: Distinct) {
        match other.estimate {
            Some(theirs) => {
                self.start_estimating();
                if let Some(mine) = &mut self.estimate {
                    for (a, b) in mine.registers.iter_mut().zip(theirs.registers) {
                        *a = (*a).max(b);
                    }
                }
            }
            None => {
                for hash in other.exact {
                    self.insert(hash);
                }
            }
        }
    }

    fn count(&self) -> u64 {
        match &self.estimate {
            Some(hll) => hll.estimate(),
            None => self.exact.len() as u64,
        }
    }

    fn estimated(&self) -> bool {
        self.estimate.is_some()
    }
}

/// HyperLogLog with 2^14 registers (about 0.8% standard error).
struct HyperLogLog {
    registers: Vec<u8>,
}

impl HyperLogLog {
    const BITS: u32 = 14;

    fn new() -> Self {
        Self {
            registers: vec![0; 1 << Self::BITS],
        }
    }

    fn insert(&mut self, hash: u64) {
        let index = (hash >> (64 - Self::BITS)) as usize;
        let rest = hash << Self::BITS;
        let rank = (rest.leading_zeros() + 1).min(64 - Self::BITS + 1) as u8;
        if rank > self.registers[index] {
            self.registers[index] = rank;
        }
    }

    fn estimate(&self) -> u64 {
        let m = self.registers.len() as f64;
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let sum: f64 = self.registers.iter().map(|&r| 2f64.powi(-(r as i32))).sum();
        let raw = alpha * m * m / sum;
        let zeros = self.registers.iter().filter(|&&r| r == 0).count();
        let estimate = if raw <= 2.5 * m && zeros > 0 {
            m * (m / zeros as f64).ln()
        } else {
            raw
        };
        estimate.round() as u64
    }
}

/// A statistics pass, detached from the document so it can run on another
/// thread. Create it with [`StatsJob::new`].
#[derive(Debug, Clone)]
pub struct StatsJob {
    pipeline: Pipeline,
    columns: Vec<String>,
}

impl StatsJob {
    /// Statistics of `columns` (all columns when empty) of the pipeline's
    /// output.
    pub fn new(pipeline: Pipeline, columns: Vec<String>) -> Result<Self> {
        let schema = pipeline.schema();
        for column in &columns {
            schema
                .index_of(column)
                .map_err(|_| invalid!(ColumnNotFound, column = column))?;
        }
        let columns = if columns.is_empty() {
            schema.fields().iter().map(|f| f.name().clone()).collect()
        } else {
            columns
        };
        Ok(Self { pipeline, columns })
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Runs the pass. `progress` gets the fraction done and returns `false`
    /// to cancel ([`Error::Cancelled`]).
    pub fn run(self, progress: &mut dyn FnMut(f32) -> bool) -> Result<Vec<ColumnStats>> {
        let schema = self.pipeline.schema();
        let indices = self
            .columns
            .iter()
            .map(|c| schema.index_of(c))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut accumulators: Vec<Accumulator> = self
            .columns
            .iter()
            .zip(&indices)
            .map(|(c, &i)| Accumulator::new(c.clone(), schema.field(i).data_type().clone()))
            .collect();
        let rows = self.pipeline.num_rows();
        // Pieces are read and counted on several threads, and the counts
        // added up in order. Once a column's distinct values are estimated,
        // later pieces estimate theirs from the start.
        let fields: Vec<(String, DataType)> = self
            .columns
            .iter()
            .zip(&indices)
            .map(|(c, &i)| (c.clone(), schema.field(i).data_type().clone()))
            .collect();
        let estimating: Arc<Vec<AtomicBool>> =
            Arc::new(fields.iter().map(|_| AtomicBool::new(false)).collect());
        let flags = estimating.clone();
        let pieces = self.pipeline.read_parallel(
            self.pipeline.evaluated(),
            0..rows,
            Some(self.columns.clone()),
            move |batches, _| {
                let mut counted = Vec::with_capacity(fields.len());
                for ((name, data_type), flag) in fields.iter().zip(flags.iter()) {
                    let mut acc = Accumulator::new(name.clone(), data_type.clone());
                    if flag.load(Ordering::Relaxed)
                        && let Some(distinct) = &mut acc.distinct
                    {
                        distinct.start_estimating();
                    }
                    counted.push(acc);
                }
                let mut rows = 0;
                for batch in &batches {
                    for acc in &mut counted {
                        let column = batch
                            .column_by_name(&acc.column)
                            .ok_or_else(|| invalid!(StatsFailed))?;
                        acc.add(column)?;
                    }
                    rows += batch.num_rows();
                }
                Ok((counted, rows))
            },
        );
        let mut done = 0;
        for piece in pieces {
            let (counted, n) = piece?;
            // Columns are added up at the same time.
            std::thread::scope(|scope| {
                for ((acc, piece), flag) in
                    accumulators.iter_mut().zip(counted).zip(estimating.iter())
                {
                    scope.spawn(move || {
                        acc.merge(piece);
                        if acc.distinct.as_ref().is_some_and(Distinct::estimated) {
                            flag.store(true, Ordering::Relaxed);
                        }
                    });
                }
            });
            done += n;
            if !progress(done as f32 / rows.max(1) as f32) {
                return Err(Error::Cancelled);
            }
        }
        Ok(accumulators.into_iter().map(Accumulator::finish).collect())
    }
}

/// Nulls, min and max of a top-level column from the Parquet footer, when
/// every row group has them. Count and the other fields stay unset (zero /
/// `None`) except `count`, which is the file's row count.
pub fn quick_stats(
    metadata: &parquet::file::metadata::ParquetMetaData,
    schema: &SchemaRef,
    column: &str,
) -> Option<ColumnStats> {
    use parquet::arrow::arrow_reader::statistics::StatisticsConverter;
    let field = schema.field_with_name(column).ok()?;
    let converter = StatisticsConverter::try_new(
        column,
        schema.as_ref(),
        metadata.file_metadata().schema_descr(),
    )
    .ok()?;
    let groups = metadata.row_groups();
    let nulls = converter.row_group_null_counts(groups.iter()).ok()?;
    if nulls.null_count() > 0 {
        return None;
    }
    let mut acc = Accumulator::new(column.to_owned(), field.data_type().clone());
    for extremes in [
        converter.row_group_mins(groups.iter()).ok()?,
        converter.row_group_maxes(groups.iter()).ok()?,
    ] {
        if extremes.null_count() > 0 && !nulls.is_empty() {
            // A row group without min/max (or with only nulls): unknown.
            let all_null_groups = (0..extremes.len())
                .filter(|&i| extremes.is_null(i))
                .all(|i| nulls.value(i) == groups[i].num_rows() as u64);
            if !all_null_groups {
                return None;
            }
        }
        if acc.orderable {
            acc.update_extremes(&extremes).ok()?;
        }
    }
    let mut stats = acc.finish();
    stats.count = groups.iter().map(|g| g.num_rows() as u64).sum();
    stats.nulls = nulls.iter().flatten().sum();
    stats.distinct = None;
    Some(stats)
}

/// A min or max in JSON: its text.
fn value_as_text<S: serde::Serializer>(
    value: &Option<ArrayRef>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let text = value
        .as_ref()
        .and_then(|a| array_value_to_string(a.as_ref(), 0).ok());
    serializer.serialize_some(&text)
}

/// Statistics as pretty JSON (for `veta-cli stats --json`).
pub fn to_json(stats: &[ColumnStats]) -> Result<String> {
    serde_json::to_string_pretty(stats).map_err(|e| invalid!(Json, error = e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{DataSource, MemorySource};
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    fn pipeline(n: i64) -> Pipeline {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("list", DataType::new_list(DataType::Int32, true), true),
        ]));
        let names = ["b", "a", "c"];
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values(0..n)),
                Arc::new(StringArray::from_iter(
                    (0..n).map(|i| (i % 4 != 3).then(|| names[(i % 3) as usize])),
                )),
                arrow::array::new_null_array(
                    &DataType::new_list(DataType::Int32, true),
                    n as usize,
                ),
            ],
        )
        .unwrap();
        let source: Arc<dyn DataSource> = Arc::new(MemorySource::new(schema, vec![batch]).unwrap());
        Pipeline::new(source)
    }

    #[test]
    fn column_stats() {
        let job = StatsJob::new(pipeline(200_000), vec![]).unwrap();
        assert_eq!(job.columns(), ["id", "name", "list"]);
        let mut seen = 0;
        let stats = job
            .run(&mut |_| {
                seen += 1;
                true
            })
            .unwrap();
        assert!(seen >= 1);
        let id = &stats[0];
        assert_eq!(
            (id.count, id.nulls, id.distinct),
            (200_000, 0, Some(200_000))
        );
        let text = |v: &Option<ArrayRef>| array_value_to_string(v.as_ref().unwrap(), 0).unwrap();
        assert_eq!(text(&id.min), "0");
        assert_eq!(text(&id.max), "199999");
        assert!((id.mean.unwrap() - 99_999.5).abs() < 1e-6);
        // Standard deviation of 0..n is about n / sqrt(12).
        assert!((id.std_dev.unwrap() - 57_735.17).abs() < 1.0);
        let name = &stats[1];
        assert_eq!(name.nulls, 50_000);
        assert_eq!(name.distinct, Some(3));
        assert!(
            name.min.is_none() && name.max.is_none(),
            "text has no range"
        );
        assert_eq!((name.mean, name.std_dev), (None, None));
        assert_eq!(stats[2].nulls, 200_000);
        assert!(stats[2].min.is_none());
    }

    #[test]
    fn unknown_column_and_cancel() {
        assert!(StatsJob::new(pipeline(10), vec!["nope".into()]).is_err());
        let job = StatsJob::new(pipeline(200_000), vec!["id".into()]).unwrap();
        assert!(matches!(job.run(&mut |_| false), Err(Error::Cancelled)));
    }

    #[test]
    fn quick_stats_agree_with_a_full_pass() {
        use crate::{Document, OpenOptions};
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("m.parquet");
        veta_testkit::fixtures::mixed_settings(&path);
        let doc = Document::open(&path, OpenOptions::default()).unwrap();
        let full = doc.stats_job(vec![]).unwrap().run(&mut |_| true).unwrap();
        for (column, stats) in ["id", "name", "flag"]
            .iter()
            .zip([&full[0], &full[1], &full[3]])
        {
            let quick = doc.quick_stats(column).unwrap();
            assert_eq!(
                (quick.count, quick.nulls, &quick.min, &quick.max),
                (stats.count, stats.nulls, &stats.min, &stats.max),
                "{column}"
            );
        }
        assert!(doc.quick_stats("name").unwrap().min.is_none());
        assert!(full[3].max.is_none(), "booleans have no range");
        // Written without statistics.
        assert!(doc.quick_stats("score").is_none());
    }

    #[test]
    fn distinct_counts_every_type_and_slice() {
        use arrow::array::{BooleanArray, Int8Array, Int32Array, StringArray};
        let count = |array: ArrayRef| {
            let converter =
                RowConverter::new(vec![SortField::new(array.data_type().clone())]).unwrap();
            let mut distinct = Distinct::default();
            insert_hashes(&array, &converter, &mut distinct).unwrap();
            distinct.count()
        };
        let bools = BooleanArray::from(vec![Some(true), None, Some(true), Some(false)]);
        assert_eq!(count(Arc::new(bools.clone())), 2);
        assert_eq!(count(Arc::new(bools.slice(0, 3))), 1, "nulls aren't values");
        assert_eq!(
            count(Arc::new(BooleanArray::from(vec![None::<bool>; 3]))),
            0
        );
        let small = Int8Array::from(vec![Some(1), Some(-1), None, Some(1), Some(0)]);
        assert_eq!(count(Arc::new(small.clone())), 3);
        assert_eq!(count(Arc::new(small.slice(2, 3))), 2);
        let ints = Int32Array::from_iter_values((0..1000).map(|i| i % 7));
        assert_eq!(count(Arc::new(ints.slice(3, 500))), 7);
        // Every length from empty to past two words, sliced.
        let words: Vec<String> = (0..20).map(|n| "x".repeat(n)).collect();
        let text = StringArray::from_iter_values(words.iter().chain(&words));
        assert_eq!(count(Arc::new(text.clone())), 20);
        assert_eq!(count(Arc::new(text.slice(5, 10))), 10);
        assert_eq!(tail_word(&[1, 2, 3]), 0x03_02_01);
        assert_eq!(tail_word(&[1, 2, 3, 4, 5, 6, 7]), 0x07_06_05_04_03_02_01);
    }

    #[test]
    fn hyperloglog_is_close() {
        let mut d = Distinct::default();
        let n = EXACT_DISTINCT_LIMIT as u64 * 2;
        for i in 0..n {
            d.insert(mix(i));
        }
        assert!(d.estimated());
        let error = (d.count() as f64 - n as f64).abs() / n as f64;
        assert!(error < 0.03, "error {error}");
    }
}
