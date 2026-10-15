//! Unusual files: deep nesting, no rows, no columns.

#![allow(clippy::unwrap_used, clippy::single_range_in_vec_init)] // Tests.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::arrow::ArrowWriter;
use veta_core::arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use veta_core::arrow::datatypes::{DataType, Field, Fields, Schema};
use veta_core::arrow::json::ReaderBuilder;
use veta_core::controller::execute;
use veta_core::display::{format_batch, value_json};
use veta_core::{Command, Document, OpenOptions};
use veta_testkit::TempDir;

fn write(path: &Path, batch: &RecordBatch) {
    let mut writer =
        ArrowWriter::try_new(File::create(path).unwrap(), batch.schema(), None).unwrap();
    writer.write(batch).unwrap();
    writer.close().unwrap();
}

fn both(path: &Path) -> Vec<Document> {
    [OpenOptions::default(), OpenOptions::paged()]
        .into_iter()
        .map(|options| Document::open(path, options).unwrap())
        .collect()
}

/// `deep: struct<items: list<struct<tags: list<string>, owner: struct<name: string>>>>`.
fn deep_schema() -> Arc<Schema> {
    let owner = DataType::Struct(Fields::from(vec![Field::new("name", DataType::Utf8, true)]));
    let tags = DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)));
    let item = DataType::Struct(Fields::from(vec![
        Field::new("tags", tags, true),
        Field::new("owner", owner, true),
    ]));
    let items = DataType::List(Arc::new(Field::new("item", item, true)));
    let deep = DataType::Struct(Fields::from(vec![Field::new("items", items, true)]));
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("deep", deep, true),
    ]))
}

#[test]
fn deeply_nested_values_read_edit_and_save() {
    let dir = TempDir::new();
    let path = dir.join("deep.parquet");
    let rows = r#"{"id": 1, "deep": {"items": [{"tags": ["a", "b"], "owner": {"name": "Ann"}}]}}
{"id": 2, "deep": {"items": []}}
{"id": 3, "deep": null}
"#;
    let batch = ReaderBuilder::new(deep_schema())
        .build(rows.as_bytes())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    write(&path, &batch);

    for mut doc in both(&path) {
        let json = |doc: &Document, row| {
            value_json(doc.read(row..row + 1).unwrap().column(1).as_ref(), 0).unwrap()
        };
        assert_eq!(
            json(&doc, 0).as_deref(),
            Some(r#"{"items":[{"tags":["a","b"],"owner":{"name":"Ann"}}]}"#)
        );
        assert_eq!(json(&doc, 2), None);
        // Edit the innermost list through the cell's JSON.
        let value = r#"{"items": [{"tags": ["c"], "owner": {"name": "Bo"}}]}"#;
        execute(
            &mut doc,
            Command::SetCell {
                row: 2,
                column: "deep".into(),
                value: Some(value.into()),
            },
        )
        .unwrap();
        let out = dir.join("deep-out.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(
            json(&saved, 2).as_deref(),
            Some(r#"{"items":[{"tags":["c"],"owner":{"name":"Bo"}}]}"#)
        );
        // Expanding every struct level stops at the list.
        execute(
            &mut doc,
            Command::ExpandStruct {
                column: "deep".into(),
                all_levels: true,
            },
        )
        .unwrap();
        doc.compute_all().unwrap();
        let names: Vec<_> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(names, ["id", "deep.items"]);
    }
}

#[test]
fn file_without_rows() {
    let dir = TempDir::new();
    let path = dir.join("empty.parquet");
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new("name", DataType::Utf8, true),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<String>::new())),
    ];
    write(&path, &RecordBatch::try_new(schema, columns).unwrap());

    for mut doc in both(&path) {
        assert_eq!(doc.num_rows(), 0);
        assert_eq!(doc.schema().fields().len(), 2);
        assert_eq!(doc.read(0..0).unwrap().num_rows(), 0);
        assert!(doc.read(0..10).unwrap().num_rows() == 0);
        execute(&mut doc, Command::InsertRows { at: 0, count: 1 }).unwrap();
        execute(
            &mut doc,
            Command::SetCell {
                row: 0,
                column: "name".into(),
                value: Some("x".into()),
            },
        )
        .unwrap();
        let out = dir.join("filled.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        let cells = format_batch(&saved.read(0..1).unwrap()).unwrap();
        assert_eq!(cells, [[None, Some("x".to_owned())]]);
    }
}

#[test]
fn file_without_columns() {
    let dir = TempDir::new();
    let path = dir.join("no-columns.parquet");
    let schema = Arc::new(Schema::empty());
    let batch = RecordBatch::new_empty(schema);
    write(&path, &batch);

    for mut doc in both(&path) {
        assert_eq!(doc.schema().fields().len(), 0);
        assert_eq!(doc.num_rows(), 0);
        execute(
            &mut doc,
            Command::AddColumn {
                name: "n".into(),
                data_type: DataType::Int64,
                at: 0,
            },
        )
        .unwrap();
        execute(&mut doc, Command::InsertRows { at: 0, count: 2 }).unwrap();
        let out = dir.join("one-column.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(saved.num_rows(), 2);
        assert_eq!(saved.schema().field(0).name(), "n");
    }
}
