//! Controller tests: setting and pasting cells.

use super::*;

#[test]
fn set_cell_parses_and_merges_into_one_step() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    execute(&mut doc, set_cell(1, "score", Some(" 2.25 "))).unwrap();
    execute(&mut doc, set_cell(2, "name", Some("  spaced "))).unwrap();
    execute(&mut doc, set_cell(3, "flag", Some("true"))).unwrap();
    execute(&mut doc, set_cell(4, "score", Some(""))).unwrap();
    execute(&mut doc, set_cell(1, "score", Some("9"))).unwrap();

    assert_eq!(doc.steps().len(), 1, "consecutive edits share a step");
    assert_eq!(cell(&doc, 1, "score").as_deref(), Some("9.0"));
    assert_eq!(cell(&doc, 2, "name").as_deref(), Some("  spaced "));
    assert_eq!(cell(&doc, 3, "flag").as_deref(), Some("true"));
    assert_eq!(cell(&doc, 4, "score"), None);

    // Undo goes back one edit at a time, including the overwrite.
    undo(&mut doc);
    assert_eq!(cell(&doc, 1, "score").as_deref(), Some("2.25"));
    for _ in 0..4 {
        undo(&mut doc);
    }
    assert!(doc.steps().is_empty());
    assert_eq!(cell(&doc, 1, "score").as_deref(), Some("0.5"));
    assert!(!doc.is_modified());
}

#[test]
fn set_cell_rejects_bad_input() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let err = execute(&mut doc, set_cell(0, "score", Some("abc"))).unwrap_err();
    assert!(err.to_string().contains("not a valid float64"), "{err}");
    assert!(
        execute(&mut doc, set_cell(0, "id", None)).is_err(),
        "required"
    );
    assert!(
        execute(&mut doc, set_cell(0, "id", Some(""))).is_err(),
        "required"
    );
    assert!(execute(&mut doc, set_cell(5000, "id", Some("1"))).is_err());
    assert!(execute(&mut doc, set_cell(0, "nope", Some("1"))).is_err());
    assert!(doc.steps().is_empty());
}

#[test]
fn copy_and_paste_struct_fields() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
    let field = |c: usize, path: &[&str]| (c, path.iter().map(|p| p.to_string()).collect());
    let text =
        crate::clipboard::copy_fields(&doc, 0..3, &[field(1, &["y"]), field(5, &["home"])], true)
            .unwrap();
    assert_eq!(
        text, "point.y\towner.home\n2\t\"{\"\"x\"\":5,\"\"y\"\":6}\"\n\t\n\t\n",
        "JSON quoted as TSV"
    );
    // Two fields of one struct in one line, and a whole column.
    let values = vec![
        vec![Some("7".into()), Some("8".into()), Some("9".into())],
        vec![Some("1".into()), None, Some("10".into())],
    ];
    execute(
        &mut doc,
        Command::PasteFields {
            row: 2,
            targets: vec![field(1, &["x"]), field(1, &["y"]), field(0, &[])],
            values,
        },
    )
    .unwrap();
    assert_eq!(cell(&doc, 2, "point").as_deref(), Some("{x: 7, y: 8}"));
    assert_eq!(cell(&doc, 2, "id").as_deref(), Some("9"));
    assert_eq!(cell(&doc, 3, "point").as_deref(), Some("{x: 1, y: null}"));
    assert!(doc.history().can_undo());
    let empty_id = Command::PasteFields {
        row: 0,
        targets: vec![field(0, &[])],
        values: vec![vec![None]],
    };
    assert!(execute(&mut doc, empty_id).is_err(), "id is required");
    let bad = Command::PasteFields {
        row: 0,
        targets: vec![field(1, &["x"])],
        values: vec![vec![Some("a".into())]],
    };
    assert!(execute(&mut doc, bad).is_err());
}

#[test]
fn paste_cells_is_one_change_and_adds_rows() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let values = vec![
        vec![Some("x".into()), Some("1,5".into())],
        vec![None, Some("2.5".into())],
    ];
    let paste = |row| Command::PasteCells {
        row,
        column: 1,
        values: values.clone(),
    };
    // "1,5" isn't a float64: nothing changes.
    assert!(execute(&mut doc, paste(0)).is_err());
    assert!(!doc.history().can_undo());
    let values = vec![
        vec![Some("x".into()), Some("1.5".into())],
        vec![None, Some("2.5".into())],
    ];
    execute(
        &mut doc,
        Command::PasteCells {
            row: 999,
            column: 1,
            values: values.clone(),
        },
    )
    .unwrap();
    assert_eq!(doc.num_rows(), 1001);
    assert_eq!(cell(&doc, 999, "name").as_deref(), Some("x"));
    assert_eq!(cell(&doc, 1000, "score").as_deref(), Some("2.5"));
    assert_eq!(cell(&doc, 1000, "name"), None);
    assert!(doc.history().can_undo());
    assert!(undo(&mut doc));
    assert_eq!(doc.num_rows(), 1000);
    assert!(!doc.history().can_undo());
    // Too wide for the columns to the right.
    assert!(
        execute(
            &mut doc,
            Command::PasteCells {
                row: 0,
                column: 3,
                values,
            }
        )
        .is_err()
    );
}
