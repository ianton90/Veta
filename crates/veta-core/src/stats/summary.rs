//! Per-column statistics: count, nulls, distinct, min, max, mean and
//! standard deviation, computed in one streaming pass over the document's
//! output.
//!
//! Distinct values are counted exactly (by 64-bit hash) up to
//! [`EXACT_DISTINCT_LIMIT`] values, then estimated with HyperLogLog. For a
//! file with no steps, [`quick_stats`] reads nulls, min and max from the
//! Parquet footer without touching the data.

use arrow::array::{Array, ArrayRef, Float64Array, make_comparator};
use arrow::compute::{SortOptions, cast};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::row::{RowConverter, SortField};
use arrow::util::display::array_value_to_string;
use serde::Serialize;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hasher};

use crate::display::type_name;
use crate::error::{Error, Result};
use crate::steps::Pipeline;

/// Rows read per batch.
const BATCH_ROWS: usize = 64 * 1024;
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
    /// Smallest and largest non-null values, as text.
    pub min: Option<String>,
    pub max: Option<String>,
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
    orderable: bool,
    /// Welford's running mean and sum of squared deviations.
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
            orderable: true,
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

    fn add(&mut self, array: &ArrayRef) -> Result<()> {
        self.count += array.len() as u64;
        self.nulls += array.logical_null_count() as u64;
        if let (Some(converter), Some(distinct)) = (&self.converter, &mut self.distinct) {
            let rows = converter.convert_columns(std::slice::from_ref(array))?;
            for i in 0..array.len() {
                if array.is_valid(i) {
                    let mut hasher = DefaultHasher::new();
                    hasher.write(rows.row(i).as_ref());
                    distinct.insert(hasher.finish());
                }
            }
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
                .ok_or_else(|| Error::InvalidCommand("expected numbers".into()))?;
            for x in values.iter().flatten().filter(|x| x.is_finite()) {
                self.n += 1;
                let delta = x - self.mean;
                self.mean += delta / self.n as f64;
                self.m2 += delta * (x - self.mean);
            }
        }
        Ok(())
    }

    fn update_extremes(&mut self, array: &ArrayRef) -> Result<()> {
        let options = SortOptions::default();
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
        let text = |a: &Option<ArrayRef>| {
            a.as_ref()
                .and_then(|a| array_value_to_string(a.as_ref(), 0).ok())
        };
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
            min: text(&self.min),
            max: text(&self.max),
            mean: numeric.then_some(self.mean),
            std_dev: (numeric && self.n > 1).then(|| (self.m2 / (self.n - 1) as f64).sqrt()),
            column: self.column,
        }
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
    exact: HashSet<u64>,
    estimate: Option<HyperLogLog>,
}

impl Distinct {
    fn insert(&mut self, hash: u64) {
        match &mut self.estimate {
            Some(hll) => hll.insert(hash),
            None => {
                self.exact.insert(hash);
                if self.exact.len() > EXACT_DISTINCT_LIMIT {
                    let mut hll = HyperLogLog::new();
                    for h in self.exact.drain() {
                        hll.insert(h);
                    }
                    self.exact.shrink_to_fit();
                    self.estimate = Some(hll);
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
                .map_err(|_| Error::InvalidCommand(format!("no column named {column:?}")))?;
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
        let mut start = 0;
        while start < rows {
            let end = (start + BATCH_ROWS).min(rows);
            let batch = self.pipeline.read(start..end)?;
            for (acc, &i) in accumulators.iter_mut().zip(&indices) {
                acc.add(batch.column(i))?;
            }
            start = end;
            if !progress(start as f32 / rows as f32) {
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
        acc.update_extremes(&extremes).ok()?;
    }
    let mut stats = acc.finish();
    stats.count = groups.iter().map(|g| g.num_rows() as u64).sum();
    stats.nulls = nulls.iter().flatten().sum();
    stats.distinct = None;
    Some(stats)
}

/// A mean or deviation for display: up to 4 decimals, trailing zeros
/// trimmed; scientific notation for very large or small values.
pub fn format_float(x: f64) -> String {
    let magnitude = x.abs();
    if magnitude != 0.0 && !(1e-4..1e15).contains(&magnitude) {
        return format!("{x:.4e}");
    }
    let text = format!("{x:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// The distinct count for display (`~` when estimated).
pub fn format_distinct(stats: &ColumnStats) -> Option<String> {
    let n = stats.distinct?;
    Some(if stats.distinct_estimated {
        format!("~{n}")
    } else {
        n.to_string()
    })
}

/// Statistics as pretty JSON (for `veta-cli stats --json`).
pub fn to_json(stats: &[ColumnStats]) -> Result<String> {
    serde_json::to_string_pretty(stats).map_err(|e| Error::InvalidCommand(e.to_string()))
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
        assert!(seen >= 4);
        let id = &stats[0];
        assert_eq!(
            (id.count, id.nulls, id.distinct),
            (200_000, 0, Some(200_000))
        );
        assert_eq!(id.min.as_deref(), Some("0"));
        assert_eq!(id.max.as_deref(), Some("199999"));
        assert!((id.mean.unwrap() - 99_999.5).abs() < 1e-6);
        // Standard deviation of 0..n is about n / sqrt(12).
        assert!((id.std_dev.unwrap() - 57_735.17).abs() < 1.0);
        let name = &stats[1];
        assert_eq!(name.nulls, 50_000);
        assert_eq!(name.distinct, Some(3));
        assert_eq!(
            (name.min.as_deref(), name.max.as_deref()),
            (Some("a"), Some("c"))
        );
        assert_eq!((name.mean, name.std_dev), (None, None));
        assert_eq!(stats[2].nulls, 200_000);
        assert_eq!(stats[2].min, None);
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
        // Written without statistics.
        assert!(doc.quick_stats("score").is_none());
    }

    #[test]
    fn float_formatting() {
        assert_eq!(format_float(99_999.5), "99999.5");
        assert_eq!(format_float(1.0 / 3.0), "0.3333");
        assert_eq!(format_float(2.0), "2");
        assert_eq!(format_float(-0.00001), "-1.0000e-5");
        assert_eq!(format_float(0.0), "0");
    }

    #[test]
    fn hyperloglog_is_close() {
        let mut d = Distinct::default();
        let n = EXACT_DISTINCT_LIMIT as u64 * 2;
        for i in 0..n {
            let mut h = DefaultHasher::new();
            h.write_u64(i);
            d.insert(h.finish());
        }
        assert!(d.estimated());
        let error = (d.count() as f64 - n as f64).abs() / n as f64;
        assert!(error < 0.03, "error {error}");
    }
}
