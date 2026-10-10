use super::filter::{Condition, Filter, FilterOp};
use super::rows::kept_ranges;
use super::*;
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
    assert!(error.contains("label"), "{error}");
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
    assert!(seen.len() >= 3 && seen.last() == Some(&1.0));
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
    assert!(!p.fail(&old_key, "boom".into()));
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
    assert!(p.fail(&key, "disk full".into()));
    assert_eq!(
        p.status(),
        &Status::Broken {
            step: 0,
            error: "disk full".into()
        }
    );
    assert_eq!(p.compute_key(), None);
    // Re-evaluating (e.g. after undo/redo) retries it.
    p.set_steps(p.steps().to_vec(), 0);
    assert_eq!(p.status(), &Status::Computing { step: 0 });
}
