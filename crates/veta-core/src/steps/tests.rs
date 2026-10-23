//! Tests for the steps and the pipeline.

use super::rows::kept_ranges;
use super::*;
use crate::error::Error;
use crate::source::DataSource;
use crate::source::MemorySource;
use arrow::array::{ArrayRef, new_null_array};
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{Field, Schema};
use arrow::record_batch::RecordBatch;
use std::sync::Arc;

fn source(rows: i64) -> Arc<dyn DataSource> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(0..rows)),
            Arc::new(StringArray::from_iter_values(
                (0..rows).map(|i| format!("n{i}")),
            )),
        ],
    )
    .unwrap();
    Arc::new(MemorySource::new(schema, vec![batch]).unwrap())
}

fn ids(batch: &RecordBatch) -> Vec<Option<i64>> {
    let column = batch.column_by_name("id").unwrap();
    let column = column.as_any().downcast_ref::<Int64Array>().unwrap();
    column.iter().collect()
}

fn names(batch: &RecordBatch) -> Vec<String> {
    batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect()
}

fn value(v: i64) -> ArrayRef {
    Arc::new(Int64Array::from(vec![v]))
}

#[test]
fn edit_cells_within_a_window() {
    let mut p = Pipeline::new(source(10));
    let mut edits = CellEdits::default();
    edits.set("id", 2, value(200));
    edits.set("id", 8, value(800));
    p.push(Step::EditCells(edits)).unwrap();

    let all: Vec<_> = ids(&p.read(0..10).unwrap()).into_iter().flatten().collect();
    assert_eq!(all, [0, 1, 200, 3, 4, 5, 6, 7, 800, 9]);
    assert_eq!(ids(&p.read(7..9).unwrap()), [Some(7), Some(800)]);
    assert_eq!(ids(&p.read(3..5).unwrap()), [Some(3), Some(4)]);
}

#[test]
fn edits_are_validated() {
    let mut p = Pipeline::new(source(3));
    let mut edits = CellEdits::default();
    edits.set("id", 5, value(1));
    assert!(p.push(Step::EditCells(edits)).is_err());

    let mut edits = CellEdits::default();
    edits.set("id", 0, new_null_array(&DataType::Int64, 1));
    assert!(p.push(Step::EditCells(edits)).is_err(), "id is required");

    let mut edits = CellEdits::default();
    edits.set("name", 0, value(1));
    assert!(p.push(Step::EditCells(edits)).is_err(), "wrong type");
    assert!(p.steps().is_empty());
}

#[test]
fn insert_rows() {
    let mut p = Pipeline::new(source(5));
    p.push(Step::InsertRows { at: 2, count: 3 }).unwrap();
    assert_eq!(p.num_rows(), 8);
    assert!(p.schema().field(0).is_nullable());
    assert_eq!(
        ids(&p.read(0..8).unwrap()),
        [
            Some(0),
            Some(1),
            None,
            None,
            None,
            Some(2),
            Some(3),
            Some(4)
        ]
    );
    assert_eq!(ids(&p.read(4..6).unwrap()), [None, Some(2)]);
    p.push(Step::InsertRows { at: 8, count: 1 }).unwrap();
    assert_eq!(ids(&p.read(7..9).unwrap()), [Some(4), None]);
    assert!(p.push(Step::InsertRows { at: 100, count: 1 }).is_err());
    // Recipes can't ask for more rows than a step may insert, or overflow.
    let code = |r: crate::error::Result<()>| match r {
        Err(Error::Invalid(e)) => Some(e.code),
        _ => None,
    };
    let too_many = Some(crate::error::ErrorCode::InsertTooMany);
    for count in [MAX_INSERT_ROWS + 1, usize::MAX] {
        assert_eq!(code(p.push(Step::InsertRows { at: 0, count })), too_many);
    }
    assert!(
        p.push(Step::InsertRows {
            at: 0,
            count: MAX_INSERT_ROWS
        })
        .is_ok()
    );
}

#[test]
fn splits_are_capped() {
    use crate::transforms::split::{MAX_SPLIT_PARTS, Split, SplitBy};
    let schema = Schema::new(vec![Field::new(
        "s",
        arrow::datatypes::DataType::Utf8,
        true,
    )]);
    let split = |by| Split {
        column: "s".into(),
        by,
    };
    let delimiter = |parts| SplitBy::Delimiter {
        delimiter: ",".into(),
        parts,
    };
    assert!(
        split(delimiter(MAX_SPLIT_PARTS))
            .output_fields(&schema)
            .is_ok()
    );
    for by in [
        delimiter(MAX_SPLIT_PARTS + 1),
        delimiter(1_000_000_000_000),
        SplitBy::Positions((1..=MAX_SPLIT_PARTS).collect()),
    ] {
        match split(by).output_fields(&schema) {
            Err(Error::Invalid(e)) => assert_eq!(e.code, crate::error::ErrorCode::SplitTooMany),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn delete_rows() {
    let mut p = Pipeline::new(source(10));
    p.push(Step::DeleteRows {
        rows: vec![1..3, 5..6, 9..10],
    })
    .unwrap();
    assert_eq!(p.num_rows(), 6);
    let all: Vec<_> = ids(&p.read(0..6).unwrap()).into_iter().flatten().collect();
    assert_eq!(all, [0, 3, 4, 6, 7, 8]);
    let middle: Vec<_> = ids(&p.read(2..5).unwrap()).into_iter().flatten().collect();
    assert_eq!(middle, [4, 6, 7]);
    assert!(p.push(Step::DeleteRows { rows: vec![0..10] }).is_err());
}

#[test]
fn column_steps() {
    let mut p = Pipeline::new(source(3));
    p.push(Step::AddColumn {
        name: "extra".into(),
        data_type: DataType::Float64,
        at: 1,
    })
    .unwrap();
    assert_eq!(names(&p.read(0..3).unwrap()), ["id", "extra", "name"]);
    assert_eq!(p.read(0..3).unwrap().column(1).null_count(), 3);

    p.push(Step::RenameColumn {
        from: "name".into(),
        to: "label".into(),
    })
    .unwrap();
    p.push(Step::MoveColumn {
        name: "label".into(),
        to: 0,
    })
    .unwrap();
    let batch = p.read(1..2).unwrap();
    assert_eq!(names(&batch), ["label", "id", "extra"]);
    let label = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(label.value(0), "n1");

    p.push(Step::RemoveColumns {
        names: vec!["extra".into(), "id".into()],
    })
    .unwrap();
    assert_eq!(names(&p.read(0..1).unwrap()), ["label"]);
    assert!(
        p.push(Step::RemoveColumns {
            names: vec!["label".into()]
        })
        .is_err()
    );
}

#[test]
fn column_names_are_validated() {
    let mut p = Pipeline::new(source(1));
    let rename = |to: &str| Step::RenameColumn {
        from: "id".into(),
        to: to.into(),
    };
    assert!(p.push(rename("name")).is_err());
    assert!(p.push(rename(" ")).is_err());
    assert!(
        p.push(Step::RenameColumn {
            from: "missing".into(),
            to: "x".into()
        })
        .is_err()
    );
    assert!(
        p.push(Step::AddColumn {
            name: "id".into(),
            data_type: DataType::Int8,
            at: 0
        })
        .is_err()
    );
}

#[test]
fn steps_compose() {
    // Delete, then insert, then edit an inserted row.
    let mut p = Pipeline::new(source(6));
    p.push(Step::DeleteRows { rows: vec![0..2] }).unwrap();
    p.push(Step::InsertRows { at: 1, count: 1 }).unwrap();
    let mut edits = CellEdits::default();
    edits.set("id", 1, value(99));
    p.push(Step::EditCells(edits)).unwrap();
    let all: Vec<_> = ids(&p.read(0..5).unwrap()).into_iter().flatten().collect();
    assert_eq!(all, [2, 99, 3, 4, 5]);

    assert!(matches!(p.pop(), Some(Step::EditCells(_))));
    assert_eq!(ids(&p.read(1..2).unwrap()), [None]);
}

#[test]
fn changing_an_earlier_step_marks_later_steps_broken() {
    let mut p = Pipeline::new(source(3));
    p.push(Step::RenameColumn {
        from: "name".into(),
        to: "label".into(),
    })
    .unwrap();
    p.push(Step::MoveColumn {
        name: "label".into(),
        to: 0,
    })
    .unwrap();
    assert_eq!(p.status(), &Status::Ready);

    // Rename to something else: the move now refers to a missing column.
    let mut steps = p.steps().to_vec();
    steps[0] = Step::RenameColumn {
        from: "name".into(),
        to: "title".into(),
    };
    p.set_steps(steps, 0);
    let Status::Broken { step, error } = p.status().clone() else {
        panic!("expected broken");
    };
    assert_eq!(step, 1);
    assert!(error.to_string().contains("label"), "{error}");
    assert_eq!(p.evaluated(), 1);
    assert_eq!(names(&p.read(0..1).unwrap()), ["id", "title"]);
    assert!(p.push(Step::InsertRows { at: 0, count: 1 }).is_err());

    // Removing the broken step fixes it.
    let mut steps = p.steps().to_vec();
    steps.pop();
    p.set_steps(steps, 1);
    assert_eq!(p.status(), &Status::Ready);
    assert!(p.push(Step::InsertRows { at: 0, count: 1 }).is_ok());
}

#[test]
fn read_at_earlier_steps() {
    let mut p = Pipeline::new(source(5));
    p.push(Step::DeleteRows { rows: vec![0..2] }).unwrap();
    p.push(Step::RemoveColumns {
        names: vec!["name".into()],
    })
    .unwrap();
    assert_eq!(p.num_rows_at(0), Some(5));
    assert_eq!(p.num_rows_at(1), Some(3));
    assert_eq!(names(&p.read_at(1, 0..1).unwrap()), ["id", "name"]);
    assert_eq!(ids(&p.read_at(0, 0..2).unwrap()), [Some(0), Some(1)]);
    assert!(p.read_at(5, 0..1).is_err());
}

#[test]
fn kept_ranges_math() {
    let deleted = vec![2..4, 6..7];
    // previous: 0 1 [2 3] 4 5 [6] 7 8 9 → output 0 1 4 5 7 8 9
    assert_eq!(kept_ranges(&deleted, 0..7), vec![0..2, 4..6, 7..10]);
    assert_eq!(kept_ranges(&deleted, 1..3), vec![1..2, 4..5]);
    assert_eq!(kept_ranges(&deleted, 4..5), vec![7..8]);
    assert_eq!(kept_ranges(&[0..3], 0..2), vec![3..5]);
}

#[test]
fn normalize() {
    assert_eq!(
        normalize_ranges(vec![5..6, 0..2, 1..3, 7..7, 6..8]),
        vec![0..3, 5..8]
    );
}

fn keep_ids_above(n: i64) -> Step {
    Step::Filter(Filter {
        conditions: vec![Condition {
            column: "id".into(),
            op: FilterOp::Greater,
            value: n.to_string(),
            case_sensitive: false,
        }],
        match_all: true,
        exclude: false,
    })
}

#[test]
fn filter_is_computed_then_read_through_its_cache() {
    let mut p = Pipeline::new(source(200_000));
    p.push(keep_ids_above(149_990)).unwrap();
    assert_eq!(p.status(), &Status::Computing { step: 0 });
    assert_eq!(p.evaluated(), 0);
    assert!(p.push(Step::InsertRows { at: 0, count: 1 }).is_err());

    let mut seen = Vec::new();
    let job = p.compute_job().unwrap();
    let computed = job
        .run(&mut |f| {
            seen.push(f);
            true
        })
        .unwrap();
    assert_eq!(seen.last(), Some(&1.0));
    assert!(p.install(computed));
    assert_eq!(p.status(), &Status::Ready);
    assert_eq!(p.num_rows(), 50_009);
    assert_eq!(ids(&p.read(0..2).unwrap()), [Some(149_991), Some(149_992)]);

    // Steps after a filter work on its output.
    p.push(Step::DeleteRows { rows: vec![0..1] }).unwrap();
    assert_eq!(ids(&p.read(0..1).unwrap()), [Some(149_992)]);
}

#[test]
fn stale_or_cancelled_results_are_dropped() {
    let mut p = Pipeline::new(source(1_000));
    p.push(keep_ids_above(10)).unwrap();
    let job = p.compute_job().unwrap();
    assert!(matches!(
        job.clone().run(&mut |_| false),
        Err(crate::Error::Cancelled)
    ));

    let old_key = job.key().to_vec();
    assert_eq!(p.compute_key().as_deref(), Some(old_key.as_slice()));
    let computed = job.run(&mut |_| true).unwrap();
    // The filter changed meanwhile: the old result no longer applies.
    p.set_steps(vec![keep_ids_above(500)], 0);
    assert!(!p.install(computed));
    assert_eq!(p.status(), &Status::Computing { step: 0 });
    assert_ne!(p.compute_key(), Some(old_key.clone()));
    assert!(!p.fail(&old_key, Arc::new(Error::Cancelled)));
    p.compute_all(&mut |_| true).unwrap();
    assert_eq!(p.num_rows(), 499);
}

#[test]
fn changing_a_step_before_a_filter_recomputes_it() {
    let mut p = Pipeline::new(source(100));
    p.push(Step::RenameColumn {
        from: "name".into(),
        to: "label".into(),
    })
    .unwrap();
    p.push(keep_ids_above(89)).unwrap();
    p.compute_all(&mut |_| true).unwrap();
    assert_eq!(p.num_rows(), 10);

    let mut steps = p.steps().to_vec();
    steps.insert(
        0,
        Step::DeleteRows {
            rows: vec![95..100],
        },
    );
    p.set_steps(steps, 0);
    assert_eq!(p.status(), &Status::Computing { step: 2 });
    p.compute_all(&mut |_| true).unwrap();
    assert_eq!(p.num_rows(), 5);

    // A filter on a removed column is broken, not computed.
    let steps = vec![
        Step::RemoveColumns {
            names: vec!["id".into()],
        },
        keep_ids_above(1),
    ];
    p.set_steps(steps, 0);
    assert!(matches!(p.status(), Status::Broken { step: 1, .. }));
}

#[test]
fn a_failed_pass_breaks_its_step() {
    let mut p = Pipeline::new(source(10));
    p.push(keep_ids_above(3)).unwrap();
    let key = p.compute_key().unwrap();
    let disk_full = Arc::new(Error::from(std::io::Error::other("disk full")));
    assert!(p.fail(&key, disk_full.clone()));
    assert_eq!(
        p.status(),
        &Status::Broken {
            step: 0,
            error: disk_full
        }
    );
    assert_eq!(p.compute_key(), None);
    // Re-evaluating (e.g. after undo/redo) retries it.
    p.set_steps(p.steps().to_vec(), 0);
    assert_eq!(p.status(), &Status::Computing { step: 0 });
}

#[test]
fn fill_carries_values_across_batches() {
    let n = 200_000usize;
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from_iter(
            (0..n).map(|i| (i % 70_000 == 5).then_some(i as i64)),
        ))],
    )
    .unwrap();
    let source: Arc<dyn DataSource> = Arc::new(MemorySource::new(schema, vec![batch]).unwrap());
    for up in [false, true] {
        let mut p = Pipeline::new(source.clone());
        p.push(Step::Fill(Fill {
            columns: vec!["id".into()],
            up,
        }))
        .unwrap();
        assert_eq!(p.status(), &Status::Computing { step: 0 });
        p.compute_all(&mut |_| true).unwrap();
        let expect = |r: usize| -> Option<i64> {
            let anchors = [5usize, 70_005, 140_005];
            if up {
                anchors.iter().find(|&&a| a >= r).map(|&a| a as i64)
            } else {
                anchors.iter().rev().find(|&&a| a <= r).map(|&a| a as i64)
            }
        };
        for r in [
            0, 4, 5, 6, 65_535, 65_536, 70_004, 70_005, 131_072, 140_006, 199_999,
        ] {
            assert_eq!(
                ids(&p.read(r..r + 1).unwrap()),
                [expect(r)],
                "row {r}, up {up}"
            );
        }
    }
}

#[test]
fn stale_temp_files_are_removed() {
    let dir = crate::steps::checkpoint::temp_dir().unwrap();
    // No process has this id (above the usual pid limits).
    let stale = dir.join("veta-4294967-0.parquet");
    std::fs::write(&stale, b"x").unwrap();
    let own = crate::steps::checkpoint::TempFile::new().unwrap();
    crate::steps::checkpoint::remove_stale_temp_files();
    if cfg!(target_os = "linux") {
        assert!(!stale.exists());
    }
    assert!(own.path().exists(), "this process's files stay");
    assert!(own.path().starts_with(&dir));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(own.path()), 0o600, "only the owner reads temp files");
        assert_eq!(mode(&dir), 0o700);
    }
    let _ = std::fs::remove_file(&stale);
}

/// A source that records which columns are read, and how.
#[derive(Debug)]
struct Tracking {
    inner: MemorySource,
    full_reads: std::sync::atomic::AtomicUsize,
    columns: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl DataSource for Tracking {
    fn schema(&self) -> arrow::datatypes::SchemaRef {
        self.inner.schema()
    }
    fn num_rows(&self) -> usize {
        self.inner.num_rows()
    }
    fn mode(&self) -> crate::source::SourceMode {
        self.inner.mode()
    }
    fn read(&self, range: Range<usize>) -> crate::error::Result<RecordBatch> {
        self.full_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.read(range)
    }
    fn read_columns(
        &self,
        range: Range<usize>,
        names: &[String],
    ) -> crate::error::Result<RecordBatch> {
        self.columns.lock().unwrap().extend(names.iter().cloned());
        self.inner.read_columns(range, names)
    }
}

#[test]
fn column_reads_take_only_what_they_need() {
    use crate::steps::{Cast, CastOptions, Condition, CustomColumn, Filter, FilterOp, Replace};
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Int64, true),
        Field::new("unused", DataType::Utf8, true),
    ]));
    let rows = 50i64;
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(0..rows)),
            Arc::new(StringArray::from_iter_values(
                (0..rows).map(|i| format!("n{i}")),
            )),
            Arc::new(Int64Array::from_iter_values((0..rows).map(|i| i * 10))),
            Arc::new(StringArray::from_iter_values((0..rows).map(|_| "x"))),
        ],
    )
    .unwrap();
    let source = Arc::new(Tracking {
        inner: MemorySource::new(schema, vec![batch]).unwrap(),
        full_reads: Default::default(),
        columns: Default::default(),
    });
    let mut p = Pipeline::new(source.clone());
    let mut edits = CellEdits::default();
    edits.set("score", 3, value(-1));
    edits.set("unused", 4, Arc::new(StringArray::from(vec!["edited"])));
    let steps = [
        Step::EditCells(edits),
        Step::InsertRows { at: 2, count: 3 },
        Step::DeleteRows { rows: vec![5..8] },
        Step::AddColumn {
            name: "extra".into(),
            data_type: DataType::Float64,
            at: 1,
        },
        Step::RenameColumn {
            from: "name".into(),
            to: "label".into(),
        },
        Step::MoveColumn {
            name: "label".into(),
            to: 0,
        },
        Step::Cast(Cast {
            column: "unused".into(),
            to: DataType::Int64,
            options: CastOptions::default(),
        }),
        Step::Replace(Replace {
            column: "label".into(),
            find: Some("n1".into()),
            with: Some("one".into()),
            whole_cell: true,
        }),
        Step::CustomColumn(CustomColumn {
            name: "double".into(),
            expression: "[score] * 2".into(),
            at: 0,
        }),
        Step::Filter(Filter {
            conditions: vec![Condition {
                column: "id".into(),
                op: FilterOp::Greater,
                value: "10".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        }),
    ];
    for step in steps {
        p.push(step).unwrap();
        while let Some(job) = p.compute_job() {
            let computed = job.run(&mut |_| true).unwrap();
            p.install(computed);
        }
    }
    assert_eq!(p.status(), &Status::Ready);
    // At every level, any set of columns reads the same as a full read.
    for level in 0..=p.steps().len() {
        let all = p.schema_at(level).unwrap();
        let rows = p.num_rows_at(level).unwrap();
        let names: Vec<String> = all.fields().iter().map(|f| f.name().clone()).collect();
        for pick in 1..(1usize << names.len()) {
            let wanted: Vec<String> = (0..names.len())
                .filter(|i| pick & (1 << i) != 0)
                .map(|i| names[i].clone())
                .collect();
            for range in [0..rows, 1..rows.min(9), rows / 2..rows] {
                let expected =
                    crate::source::project(&p.read_at(level, range.clone()).unwrap(), &wanted)
                        .unwrap();
                let got = p.read_columns_at(level, range, &wanted).unwrap();
                assert_eq!(got, expected, "level {level}, {wanted:?}");
            }
        }
    }
    // The ids after every step come from the id column alone.
    source.columns.lock().unwrap().clear();
    let before = source.full_reads.load(std::sync::atomic::Ordering::Relaxed);
    let last = p.steps().len();
    let rows = p.num_rows_at(last).unwrap();
    p.read_columns_at(last, 0..rows, &["id".to_string()])
        .unwrap();
    assert_eq!(
        source.full_reads.load(std::sync::atomic::Ordering::Relaxed),
        before
    );
    assert_eq!(
        source.columns.lock().unwrap().iter().collect::<Vec<_>>(),
        ["id"]
    );
    // A computed column reads its step's input, but no further than needed.
    source.columns.lock().unwrap().clear();
    p.read_columns_at(last, 0..rows, &["double".to_string()])
        .unwrap();
    assert_eq!(
        source.full_reads.load(std::sync::atomic::Ordering::Relaxed),
        before + 1,
        "the formula step reads its whole input"
    );
}
