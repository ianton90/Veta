//! Parquet fixture generators.
//!
//! | Fixture | Covers |
//! |---|---|
//! | [`all_types`] | every primitive type we support, with nulls |
//! | [`mixed_settings`] | several row groups, per-column compression, encoding, dictionary and statistics settings, key/value metadata |
//! | [`large`] | many rows and row groups, for paging tests |

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array,
    Int8Array, Int16Array, Int32Array, Int64Array, LargeStringArray, StringArray,
    Time64MicrosecondArray, TimestampMicrosecondArray, TimestampMillisecondArray, UInt8Array,
    UInt16Array, UInt32Array, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, GzipLevel, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::schema::types::ColumnPath;

/// Rows in [`all_types`].
pub const ALL_TYPES_ROWS: usize = 4;

/// Writes one row group with a column per primitive type. Row 2 is null in
/// every column; the other rows include edge values (min/max, empty string).
pub fn all_types(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("bool", DataType::Boolean, true),
        Field::new("i8", DataType::Int8, true),
        Field::new("i16", DataType::Int16, true),
        Field::new("i32", DataType::Int32, true),
        Field::new("i64", DataType::Int64, true),
        Field::new("u8", DataType::UInt8, true),
        Field::new("u16", DataType::UInt16, true),
        Field::new("u32", DataType::UInt32, true),
        Field::new("u64", DataType::UInt64, true),
        Field::new("f32", DataType::Float32, true),
        Field::new("f64", DataType::Float64, true),
        Field::new("utf8", DataType::Utf8, true),
        Field::new("large_utf8", DataType::LargeUtf8, true),
        Field::new("binary", DataType::Binary, true),
        Field::new("date32", DataType::Date32, true),
        Field::new("time64_us", DataType::Time64(TimeUnit::Microsecond), true),
        Field::new(
            "timestamp_ms",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        ),
        Field::new(
            "timestamp_us_utc",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new("decimal", DataType::Decimal128(10, 2), true),
    ]));

    let columns: Vec<ArrayRef> = vec![
        Arc::new(BooleanArray::from(vec![
            Some(true),
            Some(false),
            None,
            Some(true),
        ])),
        Arc::new(Int8Array::from(vec![
            Some(i8::MIN),
            Some(0),
            None,
            Some(i8::MAX),
        ])),
        Arc::new(Int16Array::from(vec![
            Some(i16::MIN),
            Some(0),
            None,
            Some(i16::MAX),
        ])),
        Arc::new(Int32Array::from(vec![
            Some(i32::MIN),
            Some(0),
            None,
            Some(i32::MAX),
        ])),
        Arc::new(Int64Array::from(vec![
            Some(i64::MIN),
            Some(0),
            None,
            Some(i64::MAX),
        ])),
        Arc::new(UInt8Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u8::MAX),
        ])),
        Arc::new(UInt16Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u16::MAX),
        ])),
        Arc::new(UInt32Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u32::MAX),
        ])),
        Arc::new(UInt64Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u64::MAX),
        ])),
        Arc::new(Float32Array::from(vec![
            Some(-1.5),
            Some(0.0),
            None,
            Some(f32::MAX),
        ])),
        Arc::new(Float64Array::from(vec![
            Some(-1.5),
            Some(0.1),
            None,
            Some(f64::MAX),
        ])),
        Arc::new(StringArray::from(vec![
            Some("alpha"),
            Some(""),
            None,
            Some("ñandú ✓"),
        ])),
        Arc::new(LargeStringArray::from(vec![
            Some("x"),
            Some("yy"),
            None,
            Some("zzz"),
        ])),
        Arc::new(BinaryArray::from(vec![
            Some(&b"\x00\x01"[..]),
            Some(&b""[..]),
            None,
            Some(&b"\xff"[..]),
        ])),
        Arc::new(Date32Array::from(vec![
            Some(0),
            Some(19_000),
            None,
            Some(-1),
        ])),
        Arc::new(Time64MicrosecondArray::from(vec![
            Some(0),
            Some(43_200_000_000),
            None,
            Some(86_399_999_999),
        ])),
        Arc::new(TimestampMillisecondArray::from(vec![
            Some(0),
            Some(1_700_000_000_000),
            None,
            Some(-1),
        ])),
        Arc::new(
            TimestampMicrosecondArray::from(vec![
                Some(0),
                Some(1_700_000_000_000_000),
                None,
                Some(1),
            ])
            .with_timezone("UTC"),
        ),
        Arc::new(
            Decimal128Array::from(vec![Some(12_345), Some(0), None, Some(-99_999_999)])
                .with_precision_and_scale(10, 2)
                .unwrap(),
        ),
    ];

    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
    write(path, &[batch], WriterProperties::default());
}

/// Rows in [`mixed_settings`].
pub const MIXED_SETTINGS_ROWS: usize = 1_000;
/// Maximum rows per row group in [`mixed_settings`].
pub const MIXED_SETTINGS_ROW_GROUP: usize = 300;
/// Key/value metadata written by [`mixed_settings`].
pub const MIXED_SETTINGS_METADATA: &[(&str, &str)] = &[
    ("owner", "veta-tests"),
    ("purpose", "writer settings round trip"),
];

/// Writes [`MIXED_SETTINGS_ROWS`] rows in row groups of
/// [`MIXED_SETTINGS_ROW_GROUP`] with different settings per column:
///
/// | Column | Compression | Encoding | Dictionary | Statistics |
/// |---|---|---|---|---|
/// | `id` (i64) | zstd(3) | delta binary packed | off | page |
/// | `name` (utf8) | snappy | plain | on | chunk |
/// | `score` (f64) | gzip(6) | byte stream split | off | none |
/// | `flag` (bool) | uncompressed | plain | off | chunk |
pub fn mixed_settings(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
        Field::new("flag", DataType::Boolean, true),
    ]));

    let n = MIXED_SETTINGS_ROWS;
    let names = ["ana", "bo", "cy", "dee", "eli"];
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from_iter_values(0..n as i64)),
        Arc::new(StringArray::from_iter(
            (0..n).map(|i| (i % 7 != 0).then(|| names[i % names.len()])),
        )),
        Arc::new(Float64Array::from_iter(
            (0..n).map(|i| (i % 11 != 0).then_some(i as f64 * 0.5)),
        )),
        Arc::new(BooleanArray::from_iter((0..n).map(|i| Some(i % 2 == 0)))),
    ];
    let batch = RecordBatch::try_new(schema, columns).unwrap();

    let col = |name: &str| ColumnPath::from(name);
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(MIXED_SETTINGS_ROW_GROUP))
        .set_key_value_metadata(Some(
            MIXED_SETTINGS_METADATA
                .iter()
                .map(|(k, v)| KeyValue::new((*k).into(), (*v).to_owned()))
                .collect(),
        ))
        // id
        .set_column_compression(col("id"), Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
        .set_column_dictionary_enabled(col("id"), false)
        .set_column_encoding(col("id"), Encoding::DELTA_BINARY_PACKED)
        .set_column_statistics_enabled(col("id"), EnabledStatistics::Page)
        // name
        .set_column_compression(col("name"), Compression::SNAPPY)
        .set_column_dictionary_enabled(col("name"), true)
        .set_column_statistics_enabled(col("name"), EnabledStatistics::Chunk)
        // score
        .set_column_compression(
            col("score"),
            Compression::GZIP(GzipLevel::try_new(6).unwrap()),
        )
        .set_column_dictionary_enabled(col("score"), false)
        .set_column_encoding(col("score"), Encoding::BYTE_STREAM_SPLIT)
        .set_column_statistics_enabled(col("score"), EnabledStatistics::None)
        // flag
        .set_column_compression(col("flag"), Compression::UNCOMPRESSED)
        .set_column_dictionary_enabled(col("flag"), false)
        .set_column_statistics_enabled(col("flag"), EnabledStatistics::Chunk)
        .build();

    write(path, &[batch], props);
}

/// Writes `rows` rows of `(id: i64, value: f64, label: utf8)` in row groups of
/// `row_group_size`, generated in batches so memory use stays bounded.
pub fn large(path: &Path, rows: usize, row_group_size: usize) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
        Field::new("label", DataType::Utf8, false),
    ]));
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(row_group_size))
        .build();

    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props)).unwrap();
    const BATCH: usize = 64 * 1024;
    let mut start = 0;
    while start < rows {
        let end = (start + BATCH).min(rows);
        let ids = start as i64..end as i64;
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values(ids.clone())),
                Arc::new(Float64Array::from_iter_values(
                    ids.clone().map(|i| i as f64 / 3.0),
                )),
                Arc::new(StringArray::from_iter_values(
                    ids.map(|i| format!("row-{i}")),
                )),
            ],
        )
        .unwrap();
        writer.write(&batch).unwrap();
        start = end;
    }
    writer.close().unwrap();
}

fn write(path: &Path, batches: &[RecordBatch], props: WriterProperties) {
    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batches[0].schema(), Some(props)).unwrap();
    for batch in batches {
        writer.write(batch).unwrap();
    }
    writer.close().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TempDir;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::file::reader::{FileReader, SerializedFileReader};

    #[test]
    fn all_types_round_trips() {
        let dir = TempDir::new();
        let path = dir.join("all_types.parquet");
        all_types(&path);

        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.map(Result::unwrap).collect();
        let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
        assert_eq!(rows, ALL_TYPES_ROWS);
        let batch = &batches[0];
        assert_eq!(batch.num_columns(), 19);
        for column in batch.columns() {
            assert_eq!(column.null_count(), 1);
            assert!(column.is_null(2));
        }
    }

    #[test]
    fn mixed_settings_has_expected_layout() {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        mixed_settings(&path);

        let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
        let meta = reader.metadata();
        assert_eq!(
            meta.file_metadata().num_rows() as usize,
            MIXED_SETTINGS_ROWS
        );
        assert_eq!(
            meta.num_row_groups(),
            MIXED_SETTINGS_ROWS.div_ceil(MIXED_SETTINGS_ROW_GROUP)
        );

        let kv = meta.file_metadata().key_value_metadata().unwrap();
        for (key, value) in MIXED_SETTINGS_METADATA {
            let entry = kv.iter().find(|e| e.key == *key).unwrap();
            assert_eq!(entry.value.as_deref(), Some(*value));
        }

        let rg = meta.row_group(0);
        let id = rg.column(0);
        assert!(matches!(id.compression(), Compression::ZSTD(_)));
        assert!(id.encodings().any(|e| e == Encoding::DELTA_BINARY_PACKED));
        assert!(id.dictionary_page_offset().is_none());

        let name = rg.column(1);
        assert_eq!(name.compression(), Compression::SNAPPY);
        assert!(name.dictionary_page_offset().is_some());

        let score = rg.column(2);
        assert!(matches!(score.compression(), Compression::GZIP(_)));
        assert!(score.encodings().any(|e| e == Encoding::BYTE_STREAM_SPLIT));
        assert!(score.statistics().is_none());

        assert_eq!(rg.column(3).compression(), Compression::UNCOMPRESSED);
    }

    #[test]
    fn large_has_requested_rows_and_row_groups() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        large(&path, 100_000, 30_000);

        let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
        let meta = reader.metadata();
        assert_eq!(meta.file_metadata().num_rows(), 100_000);
        assert_eq!(meta.num_row_groups(), 4);
    }

    #[test]
    fn temp_dir_is_removed_on_drop() {
        let dir = TempDir::new();
        let path = dir.path().to_owned();
        assert!(path.is_dir());
        drop(dir);
        assert!(!path.exists());
    }
}
