//! Files larger than one read chunk (64Ki rows) and spread over several row
//! groups: sort, filter and collect must not lose or repeat rows at chunk
//! and row group boundaries.

#![allow(clippy::unwrap_used, clippy::single_range_in_vec_init)] // Tests.

use veta_core::arrow::array::{Array, Int64Array, ListArray};
use veta_core::controller::execute;
use veta_core::steps::{Condition, CustomColumn, Filter, FilterOp, Sort, SortKey};
use veta_core::{Command, Document, OpenOptions};
use veta_testkit::{TempDir, fixtures};

const ROWS: usize = 150_000;

fn open_large(dir: &TempDir) -> Vec<Document> {
    let path = dir.join("large.parquet");
    fixtures::large(&path, ROWS, 40_000);
    [OpenOptions::default(), OpenOptions::paged()]
        .into_iter()
        .map(|options| Document::open(&path, options).unwrap())
        .collect()
}

/// The `id` column of rows `rows`.
fn ids(doc: &Document, rows: std::ops::Range<usize>) -> Vec<i64> {
    let batch = doc.read(rows).unwrap();
    let column = batch.column(batch.schema().index_of("id").unwrap()).clone();
    let ids = column.as_any().downcast_ref::<Int64Array>().unwrap();
    ids.values().to_vec()
}

fn run(doc: &mut Document, command: Command) {
    execute(doc, command).unwrap();
    doc.compute_all().unwrap();
}

#[test]
fn sort_across_chunks() {
    let dir = TempDir::new();
    for mut doc in open_large(&dir) {
        run(
            &mut doc,
            Command::Sort(Sort {
                keys: vec![SortKey {
                    column: "value".into(),
                    descending: true,
                    nulls_first: false,
                }],
            }),
        );
        assert_eq!(doc.num_rows(), ROWS);
        // Every id once, high to low, read across the chunk boundary.
        let around = 65_536 - 3..65_536 + 3;
        let expected: Vec<i64> = around.clone().map(|r| (ROWS - 1 - r) as i64).collect();
        assert_eq!(ids(&doc, around), expected);
        let all = ids(&doc, 0..ROWS);
        assert!(all.windows(2).all(|w| w[0] == w[1] + 1));
        assert_eq!(all[0], ROWS as i64 - 1);
    }
}

#[test]
fn filter_across_chunks() {
    let dir = TempDir::new();
    for mut doc in open_large(&dir) {
        run(
            &mut doc,
            Command::Filter(Filter {
                conditions: vec![Condition {
                    column: "label".into(),
                    op: FilterOp::EndsWith,
                    value: "7".into(),
                    case_sensitive: true,
                }],
                match_all: true,
                exclude: false,
            }),
        );
        assert_eq!(doc.num_rows(), ROWS / 10);
        let all = ids(&doc, 0..doc.num_rows());
        let expected: Vec<i64> = (0..ROWS as i64 / 10).map(|k| 10 * k + 7).collect();
        assert_eq!(all, expected);
    }
}

#[test]
fn collect_across_chunks() {
    let dir = TempDir::new();
    for mut doc in open_large(&dir) {
        run(
            &mut doc,
            Command::CustomColumn(CustomColumn {
                name: "group".into(),
                expression: "id % 10".into(),
                at: 0,
            }),
        );
        run(
            &mut doc,
            Command::CollectList {
                keys: vec!["group".into()],
                columns: vec!["id".into()],
            },
        );
        assert_eq!(doc.num_rows(), 10);
        let batch = doc.read(0..10).unwrap();
        let groups = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let lists = batch
            .column(1)
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        let mut seen = 0;
        for row in 0..10 {
            let group = groups.value(row);
            let items = lists.value(row);
            let items = items.as_any().downcast_ref::<Int64Array>().unwrap();
            assert_eq!(items.len(), ROWS / 10);
            assert!(items.values().iter().all(|id| id % 10 == group));
            seen += items.len();
        }
        assert_eq!(seen, ROWS);
    }
}

#[test]
fn a_last_filter_or_sort_streams_into_the_saved_file() {
    let dir = TempDir::new();
    let sort = Command::Sort(Sort {
        keys: vec![SortKey {
            column: "value".into(),
            descending: true,
            nulls_first: false,
        }],
    });
    let filter = Command::Filter(Filter {
        conditions: vec![Condition {
            column: "label".into(),
            op: FilterOp::EndsWith,
            value: "7".into(),
            case_sensitive: true,
        }],
        match_all: true,
        exclude: false,
    });
    for (k, command) in [sort, filter].into_iter().enumerate() {
        for (m, mut doc) in open_large(&dir).into_iter().enumerate() {
            // Saved while the step still needs its pass: written as it runs.
            execute(&mut doc, command.clone()).unwrap();
            let saved = dir.join(&format!("saved-{k}-{m}.parquet"));
            doc.save(Some(saved.clone())).unwrap();
            doc.compute_all().unwrap();
            let rows = doc.num_rows();
            let reopened = Document::open(&saved, OpenOptions::default()).unwrap();
            assert_eq!(reopened.num_rows(), rows);
            assert_eq!(reopened.read(0..rows).unwrap(), doc.read(0..rows).unwrap());
        }
    }
}
