//! Tests for `values::display`.

use super::*;
use arrow::array::{Int32Array, StringArray};
use arrow::datatypes::{Field, Schema};
use std::sync::Arc;

#[test]
fn formats_values_and_nulls() {
    let schema = Arc::new(Schema::new(vec![
        Field::new("a", DataType::Int32, true),
        Field::new("b", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int32Array::from(vec![Some(1), None])),
            Arc::new(StringArray::from(vec![None, Some("x")])),
        ],
    )
    .unwrap();
    let cells = format_batch(&batch).unwrap();
    assert_eq!(
        cells,
        vec![
            vec![Some("1".to_owned()), None],
            vec![None, Some("x".to_owned())]
        ]
    );
}

#[test]
fn list_and_map_items() {
    let dir = veta_testkit::TempDir::new();
    let path = dir.join("n.parquet");
    veta_testkit::fixtures::nested(&path);
    let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
    let batch = doc.read(0..4).unwrap();
    let (tags, scores) = (batch.column(2), batch.column(4));
    assert_eq!(item_count(tags.as_ref(), 0), 2);
    assert_eq!(item_count(tags.as_ref(), 1), 0);
    assert_eq!(item_count(scores.as_ref(), 0), 2);
    assert_eq!(item_count(tags.as_ref(), 3), 3);
    assert!(!has_items(&DataType::Int64));
    // Struct type names parse back, nested too.
    for field in batch.schema().fields() {
        let name = type_name(field.data_type());
        if name.starts_with("map") {
            continue;
        }
        assert_eq!(
            parse_type_name(&name).as_ref(),
            Some(field.data_type()),
            "{name}"
        );
    }
}

#[test]
fn type_names() {
    assert_eq!(type_name(&DataType::Int64), "int64");
    assert_eq!(
        type_name(&DataType::Timestamp(
            TimeUnit::Millisecond,
            Some("UTC".into())
        )),
        "timestamp[ms, UTC]"
    );
    let list = DataType::List(Arc::new(Field::new("item", DataType::Int32, true)));
    assert_eq!(type_name(&list), "list<int32>");
}

#[test]
fn parses_type_names() {
    for data_type in [
        DataType::Int64,
        DataType::Utf8,
        DataType::Boolean,
        DataType::Float32,
        DataType::Date32,
        DataType::Time64(TimeUnit::Microsecond),
        DataType::Timestamp(TimeUnit::Millisecond, None),
        DataType::Timestamp(TimeUnit::Nanosecond, Some("Europe/Madrid".into())),
        DataType::Decimal128(10, 2),
    ] {
        assert_eq!(parse_type_name(&type_name(&data_type)), Some(data_type));
    }
    assert_eq!(parse_type_name(" Double "), Some(DataType::Float64));
    assert_eq!(parse_type_name("timestamp[xs]"), None);
    let scores = map_type(DataType::Utf8, DataType::Int64);
    assert_eq!(parse_type_name("map<string, int64>"), Some(scores.clone()));
    assert_eq!(parse_type_name(&type_name(&scores)), Some(scores));
    assert_eq!(parse_type_name("map<string>"), None);
    let xyz = DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float64, true)), 3);
    assert_eq!(type_name(&xyz), "fixed_size_list<float64, 3>");
    assert_eq!(parse_type_name(&type_name(&xyz)), Some(xyz));
    assert_eq!(parse_type_name("fixed_size_list<int64, 0>"), None);
    assert_eq!(
        parse_type_name("list<int32>"),
        Some(DataType::List(Arc::new(Field::new(
            "item",
            DataType::Int32,
            true
        ))))
    );
}

mod fixture_tests {
    use super::super::*;
    use crate::{Document, OpenOptions};
    use veta_testkit::{TempDir, fixtures};

    #[test]
    fn formats_every_fixture_type() {
        let dir = TempDir::new();
        let path = dir.join("all.parquet");
        fixtures::all_types(&path);
        let doc = Document::open(&path, OpenOptions::default()).unwrap();
        let cells = format_batch(&doc.read(0..fixtures::ALL_TYPES_ROWS).unwrap()).unwrap();
        assert_eq!(cells.len(), fixtures::ALL_TYPES_ROWS);
        assert!(cells[2].iter().all(Option::is_none), "row 2 is all nulls");

        let schema = doc.schema();
        let col = |name: &str| schema.index_of(name).unwrap();
        assert_eq!(cells[0][col("utf8")].as_deref(), Some("alpha"));
        assert_eq!(cells[1][col("decimal")].as_deref(), Some("0.00"));
        assert_eq!(cells[0][col("decimal")].as_deref(), Some("123.45"));
        assert_eq!(cells[0][col("date32")].as_deref(), Some("1970-01-01"));
        assert!(
            cells[1][col("timestamp_us_utc")]
                .as_deref()
                .unwrap()
                .starts_with("2023-11-14T22:13:20")
        );
    }
}

mod nested_tests {
    use super::super::*;

    #[test]
    fn nested_cells_format() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
        let batch = doc.read(0..4).unwrap();
        let cells = format_batch(&batch).unwrap();
        assert_eq!(cells[1][1].as_deref(), Some("{x: -1, y: null}"));
    }
}
