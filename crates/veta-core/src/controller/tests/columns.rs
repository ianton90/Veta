//! Controller tests: column commands.

use super::*;

#[test]
fn keep_columns_keeps_table_order() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    execute(
        &mut doc,
        Command::KeepColumns {
            names: vec!["flag".into(), "id".into()],
        },
    )
    .unwrap();
    let names: Vec<_> = doc
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["id", "flag"]);
    assert_eq!(cell(&doc, 3, "flag").as_deref(), Some("false"));
    assert!(
        execute(
            &mut doc,
            Command::KeepColumns {
                names: vec!["nope".into()]
            }
        )
        .is_err()
    );
    assert!(execute(&mut doc, Command::KeepColumns { names: vec![] }).is_err());
}

#[test]
fn remove_empty_column_checks_every_row() {
    use crate::steps::{Cast, CastOptions, OnError, Status};
    let dir = TempDir::new();
    let cast = |column: &str, to| Cast {
        column: column.into(),
        to,
        options: CastOptions {
            on_error: OnError::ErrorColumn,
            ..CastOptions::default()
        },
    };
    let remove = |name: &str| Step::RemoveEmptyColumn { name: name.into() };

    // No errors: the error column goes.
    let mut doc = mixed(&dir);
    let steps = vec![Step::Cast(cast("id", DataType::Utf8)), remove("id errors")];
    execute(&mut doc, Command::AppendSteps { steps }).unwrap();
    assert_eq!(doc.status(), &Status::Computing { step: 1 });
    doc.compute_all().unwrap();
    assert_eq!(doc.num_columns(), 4);
    assert_eq!(cell(&doc, 2, "id").as_deref(), Some("2"));

    // Errors: the check fails, saying why.
    let mut doc = mixed(&dir);
    let steps = vec![
        Step::Cast(cast("name", DataType::Int64)),
        remove("name errors"),
    ];
    execute(&mut doc, Command::AppendSteps { steps }).unwrap();
    let err = doc.compute_all().unwrap_err().to_string();
    assert!(
        err.contains("errors isn't empty") && err.contains("\"bo\""),
        "{err}"
    );
    // The app drops the check, folded into the same undo entry.
    amend(&mut doc, Command::RemoveStep { index: 1 }).unwrap();
    assert_eq!(doc.status(), &Status::Ready);
    assert_eq!(doc.num_columns(), 5);
    assert!(undo(&mut doc));
    assert!(doc.steps().is_empty());
    assert!(!doc.history().can_undo());
}

#[test]
fn add_and_remove_struct_fields() {
    use arrow::datatypes::DataType;
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        let add = |name: &str, data_type| Command::AddField {
            column: "point".into(),
            path: Vec::new(),
            name: name.into(),
            data_type,
        };
        execute(&mut doc, add("z", DataType::Utf8)).unwrap();
        assert_eq!(
            cell(&doc, 0, "point").as_deref(),
            Some("{x: 1, y: 2, z: null}")
        );
        assert_eq!(cell(&doc, 2, "point"), None, "null structs stay null");
        assert!(
            execute(&mut doc, add("x", DataType::Utf8)).is_err(),
            "taken"
        );
        assert!(execute(&mut doc, add(" ", DataType::Utf8)).is_err());
        execute(
            &mut doc,
            set_cell(0, "point", Some(r#"{"x": 1, "y": 2, "z": "a"}"#)),
        )
        .unwrap();
        let remove = |name: &str| Command::RemoveField {
            column: "point".into(),
            path: Vec::new(),
            name: name.into(),
        };
        execute(&mut doc, remove("x")).unwrap();
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{y: 2, z: a}"));
        assert!(execute(&mut doc, remove("nope")).is_err());
        assert!(
            execute(
                &mut doc,
                Command::RemoveField {
                    column: "id".into(),
                    path: Vec::new(),
                    name: "x".into()
                }
            )
            .is_err()
        );
        assert_eq!(doc.steps()[0].describe(), "Added field point.z (string)");

        let out = dir.join("fields.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(cell(&saved, 0, "point").as_deref(), Some("{y: 2, z: a}"));
    }
}
