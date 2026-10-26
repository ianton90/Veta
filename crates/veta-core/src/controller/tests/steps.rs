//! Controller tests: transformation steps and recipes.

use super::*;

#[test]
fn step_commands_and_broken_steps() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let rename = |to: &str| {
        Command::AddStep(Step::RenameColumn {
            from: "name".into(),
            to: to.into(),
        })
    };
    execute(&mut doc, rename("label")).unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::MoveColumn {
            name: "label".into(),
            to: 0,
        }),
    )
    .unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::InsertRows { at: 0, count: 1 }),
    )
    .unwrap();

    // Removing the rename breaks the move that refers to "label".
    execute(&mut doc, Command::RemoveStep { index: 0 }).unwrap();
    assert!(matches!(
        doc.status(),
        crate::steps::Status::Broken { step: 0, .. }
    ));
    assert_eq!(doc.steps().len(), 2);
    assert!(doc.save_job(None).is_err(), "can't save while broken");
    assert!(
        execute(
            &mut doc,
            Command::AddStep(Step::InsertRows { at: 0, count: 1 })
        )
        .is_err(),
        "can't add steps while broken"
    );
    undo(&mut doc);
    assert_eq!(doc.status(), &crate::steps::Status::Ready);
    assert_eq!(doc.schema().field(0).name(), "label");

    // Move the insert before the rename, then edit the rename.
    execute(&mut doc, Command::MoveStep { from: 2, to: 0 }).unwrap();
    assert!(matches!(doc.steps()[0], Step::InsertRows { .. }));
    let Step::RenameColumn { from, .. } = doc.steps()[1].clone() else {
        panic!("expected rename");
    };
    execute(
        &mut doc,
        Command::ReplaceStep {
            index: 1,
            step: Step::RenameColumn {
                from,
                to: "title".into(),
            },
        },
    )
    .unwrap();
    assert!(matches!(
        doc.status(),
        crate::steps::Status::Broken { step: 2, .. }
    ));
    assert!(execute(&mut doc, Command::RemoveStep { index: 9 }).is_err());
}

#[test]
fn filter_command_and_save_with_pending_step() {
    use crate::steps::{Condition, Filter, FilterOp, Status};
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let filter = Filter {
        conditions: vec![Condition {
            column: "name".into(),
            op: FilterOp::Equals,
            value: "BO".into(),
            case_sensitive: false,
        }],
        match_all: true,
        exclude: false,
    };
    execute(&mut doc, Command::AddStep(Step::Filter(filter))).unwrap();
    assert_eq!(doc.status(), &Status::Computing { step: 0 });
    // Saving computes the pending step.
    let out = dir.join("out.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert!(saved.num_rows() > 0 && saved.num_rows() < 1000);
    doc.compute_all().unwrap();
    assert_eq!(doc.num_rows(), saved.num_rows());
    undo(&mut doc);
    assert_eq!(doc.num_rows(), 1000);

    let bad = Filter {
        conditions: vec![Condition {
            column: "score".into(),
            op: FilterOp::Less,
            value: "abc".into(),
            case_sensitive: false,
        }],
        match_all: true,
        exclude: false,
    };
    assert!(execute(&mut doc, Command::AddStep(Step::Filter(bad))).is_err());
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn sort_command_in_memory_and_paged() {
    use crate::steps::{Sort, SortKey, Status};
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    let sort = Sort {
        keys: vec![
            SortKey {
                column: "flag".into(),
                descending: false,
                nulls_first: false,
            },
            SortKey {
                column: "score".into(),
                descending: true,
                nulls_first: false,
            },
        ],
    };
    let mut results = Vec::new();
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        execute(&mut doc, Command::AddStep(Step::Sort(sort.clone()))).unwrap();
        assert_eq!(doc.status(), &Status::Computing { step: 0 });
        doc.compute_all().unwrap();
        assert_eq!(doc.num_rows(), 1000);
        // false first; within it, scores high to low with nulls last.
        assert_eq!(cell(&doc, 0, "flag").as_deref(), Some("false"));
        assert_eq!(cell(&doc, 0, "score").as_deref(), Some("499.5"));
        assert_eq!(cell(&doc, 499, "flag").as_deref(), Some("false"));
        assert_eq!(cell(&doc, 499, "score"), None);
        assert_eq!(cell(&doc, 500, "flag").as_deref(), Some("true"));
        // Later steps work on the sorted rows.
        execute(
            &mut doc,
            Command::AddStep(Step::DeleteRows { rows: vec![0..1] }),
        )
        .unwrap();
        assert_eq!(cell(&doc, 0, "score").as_deref(), Some("498.5"));
        let out = dir.join("sorted.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        results.push(format_batch(&saved.read(0..999).unwrap()).unwrap());
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn cast_command_with_error_column() {
    use crate::steps::{Cast, CastOptions, OnError};
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    // score (float) → text with a comma decimal, then back to a number.
    execute(
        &mut doc,
        Command::AddStep(Step::Cast(Cast {
            column: "score".into(),
            to: DataType::Utf8,
            options: CastOptions::default(),
        })),
    )
    .unwrap();
    assert_eq!(cell(&doc, 3, "score").as_deref(), Some("1.5"));
    execute(
        &mut doc,
        Command::SetCell {
            row: 3,
            column: "score".into(),
            value: Some("1.234,5".into()),
        },
    )
    .unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::Cast(Cast {
            column: "score".into(),
            to: DataType::Float64,
            options: CastOptions {
                decimal: ',',
                thousands: Some('.'),
                on_error: OnError::ErrorColumn,
                ..CastOptions::default()
            },
        })),
    )
    .unwrap();
    doc.compute_all().unwrap();
    assert_eq!(settle(&mut doc), None, "the empty column is removed");
    assert_eq!(cell(&doc, 3, "score").as_deref(), Some("1234.5"));
    // "2.5" reads as 25 with these separators; nothing fails here.
    assert_eq!(cell(&doc, 5, "score").as_deref(), Some("25.0"));
    assert!(doc.schema().field_with_name("score errors").is_err());

    // name → int: every non-null name fails into the error column.
    let mut cast = Cast {
        column: "name".into(),
        to: DataType::Int64,
        options: CastOptions {
            on_error: OnError::ErrorColumn,
            ..CastOptions::default()
        },
    };
    execute(&mut doc, Command::AddStep(Step::Cast(cast.clone()))).unwrap();
    assert!(doc.compute_all().is_err(), "the check fails");
    let kept = settle(&mut doc).expect("the column is kept");
    assert_eq!(kept.column, "name errors");
    assert!(kept.values.is_some());
    assert_eq!(doc.status(), &crate::steps::Status::Ready);
    assert_eq!(cell(&doc, 1, "name"), None);
    assert_eq!(cell(&doc, 1, "name errors").as_deref(), Some("bo"));
    let out = dir.join("cast.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(saved.schema().field(2).name(), "name errors");

    undo(&mut doc);
    cast.options.on_error = OnError::Fail;
    execute(&mut doc, Command::AddStep(Step::Cast(cast))).unwrap();
    assert!(doc.compute_all().is_err(), "failing values break the step");
    assert!(matches!(doc.status(), crate::steps::Status::Broken { .. }));
    assert!(doc.save(Some(dir.join("fail.parquet"))).is_err());
}

#[test]
fn replace_format_and_fill_commands() {
    use crate::steps::{Fill, Replace, Status, TextOp};
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        execute(
            &mut doc,
            Command::AddStep(Step::Replace(Replace {
                column: "name".into(),
                find: Some("o".into()),
                with: Some("0".into()),
                whole_cell: false,
                case_sensitive: true,
            })),
        )
        .unwrap();
        assert_eq!(cell(&doc, 1, "name").as_deref(), Some("b0"));
        execute(
            &mut doc,
            Command::AddStep(Step::FormatText {
                columns: vec!["name".into()],
                op: TextOp::Upper,
            }),
        )
        .unwrap();
        assert_eq!(cell(&doc, 2, "name").as_deref(), Some("CY"));
        execute(
            &mut doc,
            Command::AddStep(Step::Fill(Fill {
                columns: vec!["name".into(), "score".into()],
                up: false,
            })),
        )
        .unwrap();
        assert_eq!(doc.status(), &Status::Computing { step: 2 });
        doc.compute_all().unwrap();
        // Row 7's name and row 11's score were null.
        assert_eq!(cell(&doc, 7, "name").as_deref(), Some("B0"));
        assert_eq!(cell(&doc, 11, "score").as_deref(), Some("5.0"));
        assert_eq!(cell(&doc, 0, "name"), None, "nothing above row 0");
        execute(
            &mut doc,
            Command::AddStep(Step::Replace(Replace {
                column: "score".into(),
                find: None,
                with: Some("-1".into()),
                whole_cell: true,
                case_sensitive: true,
            })),
        )
        .unwrap();
        assert_eq!(cell(&doc, 0, "score").as_deref(), Some("-1.0"));
        assert!(
            execute(
                &mut doc,
                Command::AddStep(Step::FormatText {
                    columns: vec!["score".into()],
                    op: TextOp::Trim,
                }),
            )
            .is_err()
        );
    }
}

#[test]
fn custom_column_command() {
    use crate::steps::CustomColumn;
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let custom = |name: &str, expression: &str, at| {
        Command::AddStep(Step::CustomColumn(CustomColumn {
            name: name.into(),
            expression: expression.into(),
            at,
        }))
    };
    execute(&mut doc, custom("n", "row() * 10 + id", 0)).unwrap();
    assert_eq!(doc.schema().field(0).name(), "n");
    // row() counts from the step's first row, whatever window is read.
    assert_eq!(cell(&doc, 0, "n").as_deref(), Some("10"));
    assert_eq!(cell(&doc, 500, "n").as_deref(), Some("5510"));
    execute(
        &mut doc,
        custom(
            "label",
            "if(flag, upper(name), \"-\") & \"/\" & text(score)",
            5,
        ),
    )
    .unwrap();
    assert_eq!(cell(&doc, 2, "label").as_deref(), Some("CY/1.0"));
    assert_eq!(cell(&doc, 0, "label"), None, "null name and score");
    assert!(execute(&mut doc, custom("bad", "name * 2", 0)).is_err());
    assert!(execute(&mut doc, custom("n", "1", 0)).is_err());
    let out = dir.join("custom.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(saved.num_columns(), 6);
}

#[test]
fn recipe_round_trip_onto_another_document() {
    use crate::steps::{Sort, SortKey};
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    execute(
        &mut doc,
        Command::AddStep(Step::RenameColumn {
            from: "name".into(),
            to: "label".into(),
        }),
    )
    .unwrap();
    execute(
        &mut doc,
        Command::SetCell {
            row: 0,
            column: "score".into(),
            value: Some("0.1".into()),
        },
    )
    .unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::Sort(Sort {
            keys: vec![SortKey {
                column: "score".into(),
                descending: true,
                nulls_first: false,
            }],
        })),
    )
    .unwrap();
    doc.compute_all().unwrap();
    let text = crate::recipe::to_json(doc.steps(), &Default::default()).unwrap();

    let mut other = mixed(&dir);
    let steps = crate::recipe::from_json(&text, &other.schema()).unwrap();
    execute(&mut other, Command::AppendSteps { steps }).unwrap();
    other.compute_all().unwrap();
    let all = |d: &Document| format_batch(&d.read(0..1000).unwrap()).unwrap();
    assert_eq!(all(&doc), all(&other));
    // One undo removes the whole recipe.
    assert!(undo(&mut other));
    assert!(other.steps().is_empty());
    assert!(execute(&mut other, Command::AppendSteps { steps: vec![] }).is_err());
    // Replacing: the recipe applies from the source, in one undo.
    let recipe = crate::recipe::from_json(&text, &other.schema()).unwrap();
    execute(
        &mut other,
        Command::AppendSteps {
            steps: recipe.clone(),
        },
    )
    .unwrap();
    execute(
        &mut other,
        Command::AppendSteps {
            steps: recipe.clone(),
        },
    )
    .unwrap();
    assert_eq!(other.steps().len(), 2 * recipe.len());
    execute(
        &mut other,
        Command::ReplaceSteps {
            steps: recipe.clone(),
        },
    )
    .unwrap();
    assert_eq!(other.steps(), recipe.as_slice());
    assert!(undo(&mut other));
    assert_eq!(other.steps().len(), 2 * recipe.len());
    assert!(execute(&mut other, Command::ReplaceSteps { steps: vec![] }).is_err());
}

#[test]
fn failing_cast_breaks_its_step() {
    use crate::steps::{Cast, CastOptions, OnError, Status};
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    // "name" holds text like "ana": not a number.
    execute(
        &mut doc,
        Command::AddStep(Step::Cast(Cast {
            column: "name".into(),
            to: DataType::Int64,
            options: CastOptions {
                on_error: OnError::Fail,
                ..CastOptions::default()
            },
        })),
    )
    .unwrap();
    assert!(doc.compute_all().is_err());
    let Status::Broken { step, error } = doc.status() else {
        panic!("expected a broken step, got {:?}", doc.status());
    };
    assert_eq!(*step, 0);
    assert!(
        error.to_string().contains("ow 2: \"bo\" in column name"),
        "{error}"
    );
    // A cast that works is computed and read normally.
    assert!(undo(&mut doc));
    execute(
        &mut doc,
        Command::AddStep(Step::Cast(Cast {
            column: "id".into(),
            to: DataType::Utf8,
            options: CastOptions {
                on_error: OnError::Fail,
                ..CastOptions::default()
            },
        })),
    )
    .unwrap();
    doc.compute_all().unwrap();
    assert!(matches!(doc.status(), Status::Ready));
    assert_eq!(cell(&doc, 0, "id").as_deref(), Some("0"));
}
