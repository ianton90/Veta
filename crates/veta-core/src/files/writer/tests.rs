//! Tests for `files::writer`.

use super::*;
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

#[test]
fn writes_what_arrow_writer_writes() {
    let dir = veta_testkit::TempDir::new();
    let path = dir.join("p.parquet");
    let nested = veta_testkit::fixtures::deep_nested_schema();
    let deep = dir.join("deep.parquet");
    veta_testkit::fixtures::deep_nested(&deep, 250);
    let input: Vec<RecordBatch> =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&deep).unwrap())
            .unwrap()
            .with_batch_size(70)
            .build()
            .unwrap()
            .map(Result::unwrap)
            .collect();
    // Row groups of 100 rows from batches of 70: groups split batches.
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(100))
        .build();
    let schema = input[0].schema();
    let mut writer =
        ParallelWriter::new(std::fs::File::create(&path).unwrap(), schema.clone(), props).unwrap();
    for batch in &input {
        writer.write(batch).unwrap();
    }
    writer.close().unwrap();
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap()).unwrap();
    let groups: Vec<i64> = builder
        .metadata()
        .row_groups()
        .iter()
        .map(|g| g.num_rows())
        .collect();
    assert_eq!(groups, [100, 100, 50]);
    assert_eq!(
        builder.schema().fields(),
        nested.fields(),
        "Arrow schema embedded"
    );
    let output: Vec<RecordBatch> = builder.build().unwrap().map(Result::unwrap).collect();
    let read = arrow::compute::concat_batches(&schema, &output).unwrap();
    let written = arrow::compute::concat_batches(&schema, &input).unwrap();
    assert_eq!(read, written);

    // A plain table and an empty one.
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("s", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
        ],
    )
    .unwrap();
    let mut writer = ParallelWriter::new(
        std::fs::File::create(&path).unwrap(),
        schema.clone(),
        WriterProperties::default(),
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let back: Vec<RecordBatch> =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap())
            .unwrap()
            .build()
            .unwrap()
            .map(Result::unwrap)
            .collect();
    assert_eq!(back, [batch]);
    let writer = ParallelWriter::new(
        std::fs::File::create(&path).unwrap(),
        schema,
        WriterProperties::default(),
    )
    .unwrap();
    writer.close().unwrap();
    let empty =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(empty.metadata().file_metadata().num_rows(), 0);
}

#[test]
fn dictionaries_only_where_they_fit() {
    use arrow::array::Array;
    let unique = Int64Array::from_iter_values(0..1000);
    let repeated = Int64Array::from_iter_values((0..1000).map(|i| i % 10));
    let words = StringArray::from_iter_values((0..1000).map(|i| ["a", "b", "c"][i % 3]));
    let names = StringArray::from_iter_values((0..1000).map(|i| format!("name-{i}")));
    let overflows = |a: &dyn Array| dictionary_overflows(&[a], 1024);
    assert!(overflows(&unique), "1000 × 8 bytes");
    assert!(!overflows(&repeated));
    assert!(!overflows(&words));
    assert!(overflows(&names));
    // Pieces of one column count together; nulls aren't values.
    let half = |from: i64| Int64Array::from_iter_values(from..from + 100);
    assert!(!dictionary_overflows(&[&half(0), &half(0)], 1024));
    assert!(dictionary_overflows(&[&half(0), &half(100)], 1024));
    assert!(!overflows(&Int64Array::from(vec![None::<i64>; 1000])));
    // Booleans aren't dictionary-encoded.
    assert!(!overflows(&arrow::array::BooleanArray::from(vec![
        true;
        1000
    ])));

    let dir = veta_testkit::TempDir::new();
    let path = dir.join("d.parquet");
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("n", DataType::Int64, false),
        Field::new("word", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(unique),
            Arc::new(repeated),
            Arc::new(words),
            Arc::new(names),
        ],
    )
    .unwrap();
    let props = WriterProperties::builder()
        .set_dictionary_page_size_limit(1024)
        .set_column_dictionary_enabled(ColumnPath::from("word"), false)
        .build();
    let mut writer =
        ParallelWriter::new(std::fs::File::create(&path).unwrap(), schema, props).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap()).unwrap();
    let dictionary: Vec<bool> = builder
        .metadata()
        .row_group(0)
        .columns()
        .iter()
        .map(|c| c.dictionary_page_offset().is_some())
        .collect();
    assert_eq!(dictionary, [false, true, false, false], "word: set off");
    let back: Vec<RecordBatch> = builder.build().unwrap().map(Result::unwrap).collect();
    assert_eq!(back, [batch]);
}
