//! Row steps through the `DataFrame` API, eager and lazy.

#![allow(clippy::unwrap_used)] // Tests.

use std::sync::Arc;

use arrow::array::{AsArray, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Int64Type, Schema};
use arrow::record_batch::RecordBatch;
use veta_core::{DataFrame, Keep, Order, SortOptions, read_parquet};
use veta_testkit::{TempDir, fixtures};

/// group: i % 3 (null every 7th), id: i, name: "n{i % 5}".
fn frame(n: i64) -> DataFrame {
    let schema = Arc::new(Schema::new(vec![
        Field::new("group", DataType::Int64, true),
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
    ]));
    let groups: Int64Array = (0..n).map(|i| (i % 7 != 6).then_some(i % 3)).collect();
    let names: StringArray = (0..n).map(|i| Some(format!("n{}", i % 5))).collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(groups),
            Arc::new(Int64Array::from_iter_values(0..n)),
            Arc::new(names),
        ],
    )
    .unwrap();
    DataFrame::from_batches(schema, vec![batch]).unwrap()
}

/// The same frame eager and lazy.
fn modes(n: i64) -> [DataFrame; 2] {
    let df = frame(n);
    [df.clone(), df.lazy()]
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

fn ids(frame: &DataFrame) -> Vec<i64> {
    ints(frame, "id").into_iter().flatten().collect()
}

#[test]
fn sort_reverse_sample() {
    for df in modes(20) {
        let sorted = df
            .sort([("group", Order::Desc), ("id", Order::Asc)])
            .unwrap();
        assert_eq!(ints(&sorted, "group")[..3], [Some(2), Some(2), Some(2)]);
        assert_eq!(ids(&sorted)[..3], [2, 5, 8]);
        assert_eq!(ints(&sorted, "group")[18..], [None, None], "nulls last");
        let first = df
            .sort_with([("group", Order::Asc)], SortOptions { nulls_first: true })
            .unwrap();
        assert_eq!(ints(&first, "group")[0], None);
        assert_eq!(ids(&first)[2..5], [0, 3, 9], "stable");
        assert!(df.sort([("nope", Order::Asc)]).is_err());
        assert_eq!(ids(&df.reverse().unwrap())[..3], [19, 18, 17]);
        let a = df.sample(0.5, 7).unwrap();
        let b = df.sample(0.5, 7).unwrap();
        assert_eq!(ids(&a), ids(&b));
        assert_ne!(ids(&a), ids(&df.sample(0.5, 8).unwrap()));
        assert!(df.sample(1.5, 0).is_err());
        assert_eq!(df.sample(0.0, 0).unwrap().num_rows().unwrap(), 0);
        assert_eq!(df.sample(1.0, 0).unwrap().num_rows().unwrap(), 20);
    }
}

#[test]
fn insert_nulls_unique() {
    for df in modes(20) {
        let inserted = df.insert_rows(2, 3).unwrap();
        assert_eq!(inserted.num_rows().unwrap(), 23);
        assert_eq!(
            ints(&inserted, "id")[..6],
            [Some(0), Some(1), None, None, None, Some(2)]
        );
        assert!(inserted.schema().field(1).is_nullable());
        assert!(df.insert_rows(21, 1).is_err());
        let no_nulls = df.drop_nulls(["group"]).unwrap();
        assert_eq!(no_nulls.num_rows().unwrap(), 20 - 2);
        assert_eq!(inserted.drop_nulls(..).unwrap().num_rows().unwrap(), 18);
        let first = df.unique(["name"], Keep::First).unwrap();
        assert_eq!(ids(&first), [0, 1, 2, 3, 4]);
        let last = df.unique(["name"], Keep::Last).unwrap();
        assert_eq!(ids(&last), [15, 16, 17, 18, 19]);
        let once = df
            .keep_rows(0..7)
            .unwrap()
            .unique(["name"], Keep::None)
            .unwrap();
        assert_eq!(ids(&once), [2, 3, 4]);
        assert_eq!(df.unique(.., Keep::First).unwrap().num_rows().unwrap(), 20);
    }
}

#[test]
fn implode_and_buffer() {
    for df in modes(10) {
        let imploded = df
            .drop_nulls(["group"])
            .unwrap()
            .implode(["group"], ["id"])
            .unwrap();
        assert_eq!(imploded.columns(), ["group", "id"]);
        assert_eq!(ints(&imploded, "group"), [Some(0), Some(1), Some(2)]);
        let lists = imploded.column("id").unwrap().values().unwrap();
        let first: Vec<i64> = lists
            .as_list::<i32>()
            .value(0)
            .as_primitive::<Int64Type>()
            .values()
            .to_vec();
        assert_eq!(first, [0, 3, 9]);
        let buffered = df.sort([("id", Order::Desc)]).unwrap().buffer().unwrap();
        assert_eq!(ids(&buffered)[..2], [9, 8]);
        // Editing a step before the buffer recomputes it.
        let edited = buffered
            .edit_step(1, |before| before.sort([("id", Order::Asc)]))
            .unwrap();
        assert_eq!(ids(&edited)[..2], [0, 1]);
    }
}

#[test]
fn large_sorted_frames_read_and_write() {
    let dir = TempDir::new();
    let path = dir.path().join("large.parquet");
    fixtures::large(&path, 300_000, 50_000);
    for df in [
        read_parquet(&path).unwrap(),
        read_parquet(&path).unwrap().lazy(),
    ] {
        let sorted = df.sort([("id", Order::Desc)]).unwrap();
        let tail = sorted.tail(2).unwrap();
        assert_eq!(tail.column(0).as_primitive::<Int64Type>().values(), &[1, 0]);
        let out = dir.path().join(format!("sorted-{}.parquet", df.is_lazy()));
        sorted.write_parquet(&out).unwrap();
        let back = read_parquet(&out).unwrap();
        assert_eq!(ids(&back)[..2], [299_999, 299_998]);
        assert_eq!(back.num_rows().unwrap(), 300_000);
    }
}
