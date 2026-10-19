//! Tests for sorting.

use super::*;
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field};

use crate::source::DataSource;
use std::sync::Arc;

fn input(n: usize) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("group", DataType::Utf8, true),
        Field::new("score", DataType::Int64, true),
    ]));
    let groups = ["b", "a", "c"];
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values(0..n as i64)),
            Arc::new(StringArray::from_iter(
                (0..n).map(|i| (i % 10 != 9).then(|| groups[(i * 7) % 3])),
            )),
            Arc::new(Int64Array::from_iter(
                (0..n).map(|i| (i % 13 != 0).then_some(((i * 37) % 11) as i64)),
            )),
        ],
    )
    .unwrap()
}

fn key(column: &str, descending: bool) -> SortKey {
    SortKey {
        column: column.into(),
        descending,
        nulls_first: false,
    }
}

fn ints(batch: &RecordBatch, column: &str) -> Vec<Option<i64>> {
    let array = batch.column_by_name(column).unwrap();
    let array = array.as_any().downcast_ref::<Int64Array>().unwrap();
    array.iter().collect()
}

fn strings(batch: &RecordBatch, column: &str) -> Vec<Option<String>> {
    let array = batch.column_by_name(column).unwrap();
    let array = array.as_any().downcast_ref::<StringArray>().unwrap();
    array.iter().map(|s| s.map(str::to_owned)).collect()
}

/// The expected order, computed the obvious way.
fn expected(batch: &RecordBatch, sort: &Sort) -> Vec<Option<i64>> {
    let groups = strings(batch, "group");
    let scores = ints(batch, "score");
    let mut order: Vec<usize> = (0..batch.num_rows()).collect();
    order.sort_by(|&a, &b| {
        for k in &sort.keys {
            let ord = match k.column.as_str() {
                "group" => cmp_nulls_last(&groups[a], &groups[b], k.descending),
                _ => cmp_nulls_last(&scores[a], &scores[b], k.descending),
            };
            if ord.is_ne() {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    order.into_iter().map(|i| Some(i as i64)).collect()
}

fn cmp_nulls_last<T: Ord>(a: &Option<T>, b: &Option<T>, desc: bool) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) if desc => b.cmp(a),
        (Some(a), Some(b)) => a.cmp(b),
    }
}

fn sorts() -> Vec<Sort> {
    vec![
        Sort {
            keys: vec![key("score", false)],
        },
        Sort {
            keys: vec![key("group", false), key("score", true)],
        },
        Sort {
            keys: vec![key("group", true)],
        },
    ]
}

#[test]
fn in_memory_sort_is_stable_with_nulls_last() {
    let batch = input(200);
    let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
    for sort in sorts() {
        let sorted = sort_in_memory(&sort, &batch.schema(), 200, &read, 64, &mut |_| true).unwrap();
        assert_eq!(ints(&sorted, "id"), expected(&batch, &sort), "{sort:?}");
    }
}

/// Plans to try: one bucket, buckets written to files after every batch,
/// many buckets in memory.
fn plans() -> [Plan; 3] {
    [
        Plan {
            buckets: 1,
            flush_bytes: None,
        },
        Plan {
            buckets: 4,
            flush_bytes: Some(1),
        },
        Plan {
            buckets: 16,
            flush_bytes: None,
        },
    ]
}

#[test]
fn external_sort_sorts_buckets_into_a_temp_file() {
    let batch = input(1000);
    let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
    for sort in sorts() {
        for plan in plans() {
            let mut seen = Vec::new();
            let checkpoint = sort_external(
                &sort,
                &batch.schema(),
                1000,
                &read,
                &read,
                32,
                Some(plan),
                &mut |f| {
                    seen.push(f);
                    true
                },
            )
            .unwrap();
            assert_eq!(checkpoint.num_rows(), 1000);
            assert_eq!(checkpoint.schema(), batch.schema());
            let sorted = checkpoint.read(0..1000).unwrap();
            assert_eq!(
                ints(&sorted, "id"),
                expected(&batch, &sort),
                "{sort:?} {plan:?}"
            );
            assert!(seen.windows(2).all(|w| w[0] <= w[1]));
            assert_eq!(seen.last(), Some(&1.0));

            let path = checkpoint.path().to_path_buf();
            assert!(path.exists());
            drop(checkpoint);
            assert!(!path.exists(), "the checkpoint is deleted with its source");
        }
    }
}

#[test]
fn keys_only_reads_and_streams() {
    // Sampling reads only the key columns; the stream gives the same rows.
    let batch = input(5000);
    let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
    let read_keys = |r: std::ops::Range<usize>| {
        let slice = batch.slice(r.start, r.len());
        Ok(slice.project(&[1, 2])?)
    };
    for sort in sorts() {
        let rows: Vec<RecordBatch> =
            sort_stream(&sort, &batch.schema(), 5000, &read, &read_keys, 64)
                .unwrap()
                .map(Result::unwrap)
                .collect();
        let sorted = concat_batches(&batch.schema(), &rows).unwrap();
        assert_eq!(ints(&sorted, "id"), expected(&batch, &sort), "{sort:?}");
    }
    // No rows.
    let empty = sort_stream(&sorts()[0], &batch.schema(), 0, &read, &read, 64).unwrap();
    assert_eq!(empty.count(), 0);
}

#[test]
fn nulls_first_per_key() {
    let batch = input(100);
    let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
    let sort = Sort {
        keys: vec![SortKey {
            column: "score".into(),
            descending: true,
            nulls_first: true,
        }],
    };
    assert_eq!(sort.describe(), "Sorted by score descending, nulls first");
    let sorted = sort_in_memory(&sort, &batch.schema(), 100, &read, 64, &mut |_| true).unwrap();
    let scores = ints(&sorted, "score");
    let nulls = scores.iter().filter(|s| s.is_none()).count();
    assert!(nulls > 0);
    assert!(scores[..nulls].iter().all(Option::is_none));
    assert_eq!(scores[nulls], Some(10));
    // The external sort agrees.
    for plan in plans() {
        let checkpoint = sort_external(
            &sort,
            &batch.schema(),
            100,
            &read,
            &read,
            16,
            Some(plan),
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(ints(&checkpoint.read(0..100).unwrap(), "score"), scores);
    }
}

#[test]
fn external_sort_can_be_cancelled() {
    let batch = input(100);
    let read = |r: std::ops::Range<usize>| Ok(batch.slice(r.start, r.len()));
    let sort = &sorts()[0];
    let mut calls = 0;
    let result = sort_external(
        sort,
        &batch.schema(),
        100,
        &read,
        &read,
        10,
        Some(plans()[1]),
        &mut |_| {
            calls += 1;
            calls < 5
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn validation_and_description() {
    let schema = input(1).schema();
    let sort = &sorts()[1];
    assert!(sort.validate(&schema).is_ok());
    assert_eq!(
        sort.describe(),
        "Sorted by group ascending, then score descending"
    );
    assert!(Sort { keys: vec![] }.validate(&schema).is_err());
    assert!(
        Sort {
            keys: vec![key("nope", false)]
        }
        .validate(&schema)
        .is_err()
    );
}
