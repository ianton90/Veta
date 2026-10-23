//! Per-column statistics: count, nulls, distinct, min, max, mean and
//! standard deviation (of numbers, dates, times and durations), and text
//! lengths, computed in one streaming pass over the document's output.
//!
//! Distinct values are counted exactly (by 64-bit hash) up to
//! [`EXACT_DISTINCT_LIMIT`] values, then estimated with HyperLogLog. For a
//! file with no steps, [`quick_stats`] reads nulls, min and max from the
//! Parquet footer without touching the data.

use arrow::array::{Array, ArrayRef};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::util::display::array_value_to_string;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::accumulator::Accumulator;
use super::distinct::Distinct;
use crate::error::{Error, Result, invalid};
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
    /// Mean, as a one-value array: a float for numbers; for dates, times,
    /// timestamps and durations, in the column's type. A number in JSON
    /// for numbers, text otherwise.
    #[serde(serialize_with = "value_as_json")]
    pub mean: Option<ArrayRef>,
    /// Sample standard deviation, like the mean; a duration for dates,
    /// times and timestamps.
    #[serde(serialize_with = "value_as_json")]
    pub std_dev: Option<ArrayRef>,
    /// Lengths of text values, in characters.
    pub text_lengths: Option<TextLengths>,
}

/// Lengths of a text column's values, in characters.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextLengths {
    pub min: u64,
    pub max: u64,
    pub mean: f64,
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

/// A mean or spread in JSON: a number for floats, else its text.
fn value_as_json<S: serde::Serializer>(
    value: &Option<ArrayRef>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    use arrow::array::AsArray;
    match value {
        Some(v) if v.data_type() == &DataType::Float64 && v.is_valid(0) => {
            serializer.serialize_some(&v.as_primitive::<arrow::datatypes::Float64Type>().value(0))
        }
        other => value_as_text(other, serializer),
    }
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
        assert!((float(&id.mean) - 99_999.5).abs() < 1e-6);
        // Standard deviation of 0..n is about n / sqrt(12).
        assert!((float(&id.std_dev) - 57_735.17).abs() < 1.0);
        let name = &stats[1];
        assert_eq!(name.nulls, 50_000);
        assert_eq!(name.distinct, Some(3));
        assert!(
            name.min.is_none() && name.max.is_none(),
            "text has no range"
        );
        assert!(name.mean.is_none() && name.std_dev.is_none());
        let lengths = name.text_lengths.as_ref().unwrap();
        assert_eq!((lengths.min, lengths.max, lengths.mean), (1, 1, 1.0));
        assert_eq!(stats[2].nulls, 200_000);
        assert!(stats[2].min.is_none());
    }

    fn float(value: &Option<ArrayRef>) -> f64 {
        use arrow::array::AsArray;
        let value = value.as_ref().unwrap();
        value
            .as_primitive::<arrow::datatypes::Float64Type>()
            .value(0)
    }

    #[test]
    fn dates_times_and_text() {
        use arrow::array::{Date32Array, TimestampSecondArray};
        let schema = Arc::new(Schema::new(vec![
            Field::new("day", DataType::Date32, true),
            Field::new(
                "at",
                DataType::Timestamp(arrow::datatypes::TimeUnit::Second, None),
                true,
            ),
            Field::new("word", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                // 1970-01-01, -03, -05 and a null.
                Arc::new(Date32Array::from(vec![Some(0), Some(2), Some(4), None])),
                Arc::new(TimestampSecondArray::from(vec![0, 60, 120, 180])),
                Arc::new(StringArray::from(vec![
                    Some("añb"),
                    Some(""),
                    None,
                    Some("abcdef"),
                ])),
            ],
        )
        .unwrap();
        let source: Arc<dyn DataSource> = Arc::new(MemorySource::new(schema, vec![batch]).unwrap());
        let stats = StatsJob::new(Pipeline::new(source), vec![])
            .unwrap()
            .run(&mut |_| true)
            .unwrap();
        let text = |v: &Option<ArrayRef>| array_value_to_string(v.as_ref().unwrap(), 0).unwrap();
        assert_eq!(text(&stats[0].mean), "1970-01-03");
        assert_eq!(
            stats[0].std_dev.as_ref().unwrap().data_type(),
            &DataType::Duration(arrow::datatypes::TimeUnit::Second)
        );
        // Two days, in seconds.
        assert_eq!(text(&stats[0].std_dev), "PT172800S");
        assert_eq!(text(&stats[1].mean), "1970-01-01T00:01:30");
        let lengths = stats[2].text_lengths.as_ref().unwrap();
        assert_eq!(
            (lengths.min, lengths.max, lengths.mean),
            (0, 6, 3.0),
            "characters, not bytes"
        );
        let json = to_json(&stats).unwrap();
        assert!(json.contains(r#""mean": "1970-01-03""#), "{json}");
    }

    #[test]
    fn huge_floats_keep_a_mean() {
        use arrow::array::Float64Array;
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, true)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Float64Array::from(vec![
                f64::MAX,
                f64::MAX,
                -f64::MAX,
            ]))],
        )
        .unwrap();
        let source: Arc<dyn DataSource> = Arc::new(MemorySource::new(schema, vec![batch]).unwrap());
        let stats = StatsJob::new(Pipeline::new(source), vec![])
            .unwrap()
            .run(&mut |_| true)
            .unwrap();
        let mean = float(&stats[0].mean);
        assert!((mean - f64::MAX / 3.0).abs() / f64::MAX < 1e-12, "{mean}");
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
}
