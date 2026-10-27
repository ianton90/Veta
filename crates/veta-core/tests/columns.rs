//! Column steps through the `DataFrame` API, eager and lazy.

#![allow(clippy::unwrap_used)] // Tests.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, AsArray, Float64Array, Int64Array, ListArray, StringArray, StructArray,
};
use arrow::datatypes::{DataType, Field, Fields, Float64Type, Int64Type, Schema};
use arrow::record_batch::RecordBatch;
use veta_core::{
    CastOptions, DataFrame, Fill, Null, OnError, PathPart, ReplaceOptions, SplitBy, TextOp,
};
use veta_testkit::TempDir;

/// id 0..n, name "Name i" (null every 4th), amount i/2 as text (null
/// every 5th), point {x: i}, tags [i, i+1] (empty every 3rd).
fn frame(n: i64) -> DataFrame {
    let point = Fields::from(vec![Field::new("x", DataType::Int64, true)]);
    let item = Arc::new(Field::new("item", DataType::Int64, true));
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("amount", DataType::Utf8, true),
        Field::new("point", DataType::Struct(point.clone()), true),
        Field::new("tags", DataType::List(item.clone()), true),
    ]));
    let ids = Int64Array::from_iter_values(0..n);
    let names: StringArray = (0..n)
        .map(|i| (i % 4 != 0).then(|| format!("Name {i}")))
        .collect();
    let amounts: StringArray = (0..n)
        .map(|i| (i % 5 != 0).then(|| format!("{}", i as f64 / 2.0)))
        .collect();
    let xs: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n));
    let points = StructArray::new(point, vec![xs], None);
    let tags = ListArray::from_iter_primitive::<Int64Type, _, _>((0..n).map(|i| {
        Some(if i % 3 == 0 {
            vec![]
        } else {
            vec![Some(i), Some(i + 1)]
        })
    }));
    let _ = item;
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(names),
            Arc::new(amounts),
            Arc::new(points),
            Arc::new(tags),
        ],
    )
    .unwrap();
    DataFrame::from_batches(schema, vec![batch]).unwrap()
}

fn ints(frame: &DataFrame, column: &str) -> Vec<Option<i64>> {
    frame
        .column(column)
        .unwrap()
        .values()
        .unwrap()
        .as_primitive::<Int64Type>()
        .iter()
        .collect()
}

fn texts(frame: &DataFrame, column: &str) -> Vec<Option<String>> {
    let values = frame.column(column).unwrap().values().unwrap();
    let values = arrow::compute::cast(&values, &DataType::Utf8).unwrap();
    values
        .as_string::<i32>()
        .iter()
        .map(|s| s.map(str::to_owned))
        .collect()
}

/// The same frame, eager and lazy.
fn both(n: i64) -> [DataFrame; 2] {
    let eager = frame(n);
    [eager.clone(), eager.lazy()]
}

#[test]
fn rename_move_reorder_insert() {
    for df in both(10) {
        let renamed = df.rename([("id", "key"), ("name", "who")]).unwrap();
        assert_eq!(renamed.columns(), ["key", "who", "amount", "point", "tags"]);
        assert!(df.rename([("id", "name")]).is_err());
        assert!(df.rename([("nope", "x")]).is_err());
        let moved = df.move_columns(["tags", "id"], 1).unwrap();
        assert_eq!(moved.columns(), ["name", "tags", "id", "amount", "point"]);
        assert_eq!(ints(&moved, "id"), ints(&df, "id"));
        assert_eq!(df.move_column("id", 4).unwrap().columns()[4], "id");
        let order = df
            .reorder_columns(["point", "tags", "amount", "name", "id"])
            .unwrap();
        assert_eq!(order.columns(), ["point", "tags", "amount", "name", "id"]);
        assert!(df.reorder_columns(["id", "name"]).is_err(), "every column");
        let inserted = df.insert_column("note", DataType::Utf8, 2).unwrap();
        assert_eq!(inserted.columns()[2], "note");
        assert_eq!(texts(&inserted, "note"), vec![None; 10]);
        assert_eq!(ints(&inserted, "id"), ints(&df, "id"));
        assert!(df.insert_column("id", DataType::Utf8, 0).is_err());
        assert!(df.insert_column("x", DataType::Utf8, 9).is_err());
    }
}

#[test]
fn cast_fill_replace_format() {
    for df in both(20) {
        let cast = df
            .cast("amount", DataType::Float64, CastOptions::default())
            .unwrap();
        let amounts = cast.column("amount").unwrap().values().unwrap();
        let amounts: Vec<Option<f64>> = amounts.as_primitive::<Float64Type>().iter().collect();
        assert_eq!(amounts[..3], [None, Some(0.5), Some(1.0)]);
        // Values that don't convert: null, or an error, or an error column.
        let words = df
            .replace("amount", "1", "one", ReplaceOptions::default())
            .unwrap();
        let options = CastOptions {
            on_error: OnError::ErrorColumn,
            ..Default::default()
        };
        let with_errors = words.cast("amount", DataType::Float64, options).unwrap();
        assert_eq!(with_errors.columns()[3], "amount errors");
        assert_eq!(
            texts(&with_errors, "amount errors")[2],
            Some("one".to_owned())
        );
        let options = CastOptions {
            on_error: OnError::Fail,
            ..Default::default()
        };
        let failing = words.cast("amount", DataType::Float64, options);
        match df.is_lazy() {
            false => assert!(failing.is_err(), "eager runs the step now"),
            true => assert!(failing.unwrap().column("amount").unwrap().values().is_err()),
        }
        let down = df.fill_null("name", Fill::Down).unwrap();
        assert_eq!(
            texts(&down, "name")[..5],
            [
                None,
                Some("Name 1".into()),
                Some("Name 2".into()),
                Some("Name 3".into()),
                Some("Name 3".into())
            ]
        );
        let up = df.fill_null("name", Fill::Up).unwrap();
        assert_eq!(texts(&up, "name")[0], Some("Name 1".into()));
        let value = df.fill_null("name", Fill::value("?")).unwrap();
        assert_eq!(texts(&value, "name")[0], Some("?".into()));
        assert!(df.fill_null("id", Fill::value("x")).is_err());
        let replaced = df
            .replace("name", "name", "N", ReplaceOptions::default())
            .unwrap();
        assert_eq!(
            texts(&replaced, "name")[1],
            Some("Name 1".into()),
            "case sensitive"
        );
        let options = ReplaceOptions {
            case_sensitive: false,
            ..Default::default()
        };
        let replaced = df.replace("name", "name", "N", options).unwrap();
        assert_eq!(texts(&replaced, "name")[1], Some("N 1".into()));
        let nulls = df
            .replace("name", Null, "nobody", ReplaceOptions::default())
            .unwrap();
        assert_eq!(texts(&nulls, "name")[0], Some("nobody".into()));
        let whole = ReplaceOptions {
            whole_cell: true,
            ..Default::default()
        };
        assert_eq!(
            texts(&df.replace("name", "Name", "x", whole).unwrap(), "name")[1],
            Some("Name 1".into())
        );
        let upper = df.format_text(["name"], TextOp::Upper).unwrap();
        assert_eq!(texts(&upper, "name")[1], Some("NAME 1".into()));
        assert!(df.format_text(["id"], TextOp::Upper).is_err());
    }
}

#[test]
fn split_merge_nested_shift() {
    for df in both(12) {
        let split = df
            .split(
                "name",
                SplitBy::Delimiter {
                    delimiter: " ".into(),
                    parts: 2,
                },
            )
            .unwrap();
        assert_eq!(split.columns()[1..3], ["name.1", "name.2"]);
        assert_eq!(texts(&split, "name.2")[1], Some("1".into()));
        let merged = df.merge_columns(["name", "amount"], "|", "both").unwrap();
        assert_eq!(merged.columns(), ["id", "both", "point", "tags"]);
        assert_eq!(texts(&merged, "both")[1], Some("Name 1|0.5".into()));
        let unnested = df.unnest("point").unwrap();
        assert_eq!(unnested.columns()[3], "point.x");
        assert_eq!(ints(&unnested, "point.x"), ints(&df, "id"));
        let added = df.add_field("point", &[], "y", DataType::Float64).unwrap();
        let DataType::Struct(fields) = added.schema().field(3).data_type().clone() else {
            panic!()
        };
        assert_eq!(fields.len(), 2);
        let removed = added.remove_field("point", &[], "y").unwrap();
        assert_eq!(removed.schema().field(3), df.schema().field(3));
        let _ = PathPart::Index(0);
        let exploded = df.explode("tags").unwrap();
        // 4 empty lists give a row each; 8 lists give 2.
        assert_eq!(exploded.num_rows().unwrap(), 4 + 8 * 2);
        assert_eq!(
            ints(&exploded, "tags")[..4],
            [None, Some(1), Some(2), Some(2)]
        );
        let shifted = df.shift("id", 2).unwrap();
        assert_eq!(ints(&shifted, "id")[..4], [None, None, Some(0), Some(1)]);
        let up = df.shift_all(-3).unwrap();
        assert_eq!(ints(&up, "id")[8..], [Some(11), None, None, None]);
        assert_eq!(texts(&up, "name")[0], Some("Name 3".into()));
    }
}

#[test]
fn large_derived_columns_spill_and_write() {
    let dir = TempDir::new();
    let df = frame(200_000)
        .cast("amount", DataType::Float64, CastOptions::default())
        .unwrap()
        .shift("id", 1)
        .unwrap();
    let path = dir.path().join("out.parquet");
    df.write_parquet(&path, &df.writer_settings()).unwrap();
    let back = veta_core::read_parquet(&path, false).unwrap();
    assert_eq!(back.num_rows().unwrap(), 200_000);
    assert_eq!(ints(&back, "id")[..3], [None, Some(0), Some(1)]);
    let lazy = frame(200_000).lazy().shift("id", 1).unwrap();
    assert_eq!(
        ints(&lazy, "id")[65_535..65_538],
        [Some(65_534), Some(65_535), Some(65_536)],
        "across pieces"
    );
    let _ = Float64Array::from(vec![1.0]);
}
