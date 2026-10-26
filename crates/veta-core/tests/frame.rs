//! The `DataFrame` API as a library user calls it: reading, inspecting,
//! picking rows and columns, writing.

#![allow(clippy::unwrap_used)] // Tests.

use arrow::array::{Array, AsArray};
use arrow::datatypes::Int64Type;
use veta_core::{DataFrame, ReadOptions, read_csv, read_parquet, read_parquet_with};
use veta_testkit::{TempDir, fixtures};

const ROWS: usize = 300_000;

fn ids(frame: &DataFrame) -> Vec<i64> {
    let values = frame.column("id").unwrap().values().unwrap();
    values.as_primitive::<Int64Type>().values().to_vec()
}

#[test]
fn eager_and_lazy_read_the_same() {
    let dir = TempDir::new();
    let path = dir.path().join("large.parquet");
    fixtures::large(&path, ROWS, 50_000);
    let eager = read_parquet(&path).unwrap();
    let lazy = read_parquet_with(
        &path,
        ReadOptions {
            lazy: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!eager.is_lazy() && lazy.is_lazy());
    for frame in [&eager, &lazy] {
        assert_eq!(frame.num_rows().unwrap(), ROWS);
        assert_eq!(frame.columns(), ["id", "value", "label"]);
        assert_eq!(ids(frame), (0..ROWS as i64).collect::<Vec<_>>());
        let tail = frame.tail(2).unwrap();
        assert_eq!(
            tail.column(0).as_primitive::<Int64Type>().values(),
            &[299_998, 299_999]
        );
    }
    let some = read_parquet_with(
        &path,
        ReadOptions {
            columns: Some(vec!["label".into(), "id".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(some.columns(), ["label", "id"]);
    let label = some.value(7, "label").unwrap();
    assert_eq!(label.as_string::<i32>().value(0), "row-7");
    assert!(
        read_parquet_with(
            &path,
            ReadOptions {
                columns: Some(vec!["nope".into()]),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn rows_and_columns() {
    let dir = TempDir::new();
    let path = dir.path().join("large.parquet");
    fixtures::large(&path, ROWS, 50_000);
    let lazy = read_parquet_with(
        &path,
        ReadOptions {
            lazy: true,
            ..Default::default()
        },
    )
    .unwrap();
    let df = read_parquet(&path).unwrap();
    for df in [df, lazy] {
        // A mask keeping every third row, then a range of what is left.
        let mask: Vec<bool> = (0..ROWS).map(|i| i.is_multiple_of(3)).collect();
        let thirds = df.keep_rows(mask).unwrap();
        assert_eq!(thirds.num_rows().unwrap(), ROWS / 3);
        let some = thirds.slice(10..15, ["id", "label"]).unwrap();
        assert_eq!(some.columns(), ["id", "label"]);
        assert_eq!(ids(&some), [30, 33, 36, 39, 42]);
        // A list of rows, in any order, each once.
        let picked = df.keep_rows([299_999, 5, 5, 100_000]).unwrap();
        assert_eq!(ids(&picked), [5, 100_000, 299_999]);
        let dropped = df.drop_rows(1..ROWS - 1).unwrap();
        assert_eq!(ids(&dropped), [0, ROWS as i64 - 1]);
        // Columns by position, by mask, dropped.
        assert_eq!(df.keep_columns([2, 0]).unwrap().columns(), ["label", "id"]);
        assert_eq!(
            df.slice(.., vec![false, true, true]).unwrap().columns(),
            ["value", "label"]
        );
        assert_eq!(df.drop_columns("value").unwrap().columns(), ["id", "label"]);
        // The frame these came from is unchanged.
        assert_eq!(df.num_rows().unwrap(), ROWS);
        assert_eq!(df.num_columns(), 3);
        // Wrong rows and columns are refused when the step is added.
        assert!(df.keep_rows(0..ROWS + 1).is_err());
        assert!(df.keep_rows(vec![true; 3]).is_err());
        assert!(df.keep_columns(["id", "id"]).is_err());
        assert!(df.keep_columns([3]).is_err());
        assert!(df.column("nope").is_err());
        assert!(df.value(ROWS, 0).is_err());
    }
}

#[test]
fn a_later_source_replaces_the_data() {
    let dir = TempDir::new();
    let (large, mixed) = (
        dir.path().join("large.parquet"),
        dir.path().join("all.parquet"),
    );
    fixtures::large(&large, 1000, 1000);
    fixtures::all_types(&mixed);
    let first = read_parquet(&large).unwrap().keep_rows(0..10).unwrap();
    let second = first.read_parquet(&mixed).unwrap();
    assert_eq!(first.num_rows().unwrap(), 10);
    assert_ne!(second.columns(), first.columns());
    assert_eq!(second.schema(), read_parquet(&mixed).unwrap().schema());
}

#[test]
fn writes_and_reads_back() {
    let dir = TempDir::new();
    let path = dir.path().join("large.parquet");
    fixtures::large(&path, 10_000, 4_000);
    let df = read_parquet(&path).unwrap().drop_rows(0..5000).unwrap();
    let parquet = dir.path().join("out.parquet");
    df.write_parquet(&parquet).unwrap();
    let back = read_parquet(&parquet).unwrap();
    assert_eq!(back.schema(), df.schema());
    assert_eq!(ids(&back), (5000..10_000).collect::<Vec<_>>());
    let csv = dir.path().join("out.csv");
    df.write_csv(&csv).unwrap();
    assert_eq!(ids(&read_csv(&csv).unwrap()), ids(&df));
    let json = dir.path().join("out.json");
    df.write_ndjson(&json).unwrap();
    assert_eq!(ids(&veta_core::read_json(&json).unwrap()), ids(&df));
    let empty = DataFrame::new();
    assert_eq!((empty.num_rows().unwrap(), empty.num_columns()), (0, 0));
    assert_eq!(df.column("id").unwrap().values().unwrap().len(), 5000);
}
