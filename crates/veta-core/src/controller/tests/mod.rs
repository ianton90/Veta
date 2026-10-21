//! Tests for the controller, by area; undo, redo and general commands here.

use super::*;
use crate::OpenOptions;
use crate::display::format_batch;
use arrow::datatypes::DataType;
use veta_testkit::{TempDir, fixtures};

mod cells;
mod columns;
mod metadata;
mod nested;
mod steps;

fn set(key: &str, value: &str) -> Command {
    Command::SetMetadata {
        key: key.into(),
        value: Some(value.into()),
    }
}

fn remove(key: &str) -> Command {
    Command::RemoveMetadata { key: key.into() }
}

fn value<'a>(doc: &'a Document, key: &str) -> Option<&'a str> {
    doc.metadata().get(key)?.value.as_deref()
}

#[test]
fn rejected_commands_change_nothing() {
    let mut doc = Document::new();
    let err = execute(&mut doc, set("", "x")).unwrap_err();
    assert_eq!(err.code(), Some(crate::error::ErrorCode::MetadataKeyEmpty));
    assert!(doc.metadata().key_value.is_empty());
    assert!(execute(&mut doc, remove("missing")).is_err());
    assert!(!doc.history().can_undo());
    assert!(!doc.is_modified());
}

#[test]
fn undo_and_redo() {
    let mut doc = Document::new();
    execute(&mut doc, set("owner", "a")).unwrap();
    execute(&mut doc, set("owner", "b")).unwrap();
    execute(&mut doc, remove("owner")).unwrap();
    assert!(doc.history().can_undo());

    assert!(undo(&mut doc));
    assert_eq!(value(&doc, "owner"), Some("b"));
    assert!(undo(&mut doc));
    assert_eq!(value(&doc, "owner"), Some("a"));
    assert!(undo(&mut doc));
    assert_eq!(value(&doc, "owner"), None);
    assert!(!undo(&mut doc));

    assert!(redo(&mut doc).unwrap());
    assert_eq!(value(&doc, "owner"), Some("a"));
    assert!(doc.history().can_redo());

    // A new change discards the redo stack.
    execute(&mut doc, set("other", "x")).unwrap();
    assert!(!doc.history().can_redo());
    assert!(!redo(&mut doc).unwrap());
}

#[test]
fn modified_tracks_the_saved_state() {
    let mut doc = Document::new();
    assert!(!doc.is_modified());
    execute(&mut doc, set("a", "1")).unwrap();
    assert!(doc.is_modified());
    undo(&mut doc);
    assert!(!doc.is_modified(), "back to the opened state");

    execute(&mut doc, set("a", "1")).unwrap();
    doc.history.mark_saved();
    assert!(!doc.is_modified());
    undo(&mut doc);
    assert!(doc.is_modified());
    redo(&mut doc).unwrap();
    assert!(!doc.is_modified());

    // Undo past the save point and branch: the saved state is gone.
    undo(&mut doc);
    execute(&mut doc, set("b", "2")).unwrap();
    assert!(doc.is_modified());
    undo(&mut doc);
    assert!(doc.is_modified());
}

/// Opens the mixed fixture: id (int64, required), name (utf8), score
/// (float64), flag (bool); 1000 rows.
fn mixed(dir: &TempDir) -> Document {
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    Document::open(&path, OpenOptions::default()).unwrap()
}

fn cell(doc: &Document, row: usize, column: &str) -> Option<String> {
    let index = doc.schema().index_of(column).unwrap();
    format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][index].clone()
}

fn set_cell(row: usize, column: &str, value: Option<&str>) -> Command {
    Command::SetCell {
        row,
        column: column.into(),
        value: value.map(str::to_owned),
    }
}

#[test]
fn row_and_column_commands_undo() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let commands = vec![
        Command::InsertRows { at: 0, count: 2 },
        Command::DeleteRows {
            rows: vec![10..20, 5..12],
        },
        Command::AddColumn {
            name: "new".into(),
            data_type: DataType::Int32,
            at: 4,
        },
        Command::RenameColumn {
            from: "name".into(),
            to: "label".into(),
        },
        Command::MoveColumn {
            name: "flag".into(),
            to: 0,
        },
        Command::RemoveColumns {
            names: vec!["score".into()],
        },
    ];
    for command in commands {
        execute(&mut doc, command).unwrap();
    }
    assert_eq!(doc.num_rows(), 1000 + 2 - 15);
    let names: Vec<_> = doc
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["flag", "id", "label", "new"]);
    assert_eq!(cell(&doc, 0, "id"), None, "inserted row");
    assert_eq!(cell(&doc, 5, "id").as_deref(), Some("18"));

    while undo(&mut doc) {}
    assert_eq!(doc.num_rows(), 1000);
    assert_eq!(doc.num_columns(), 4);
    assert_eq!(cell(&doc, 0, "id").as_deref(), Some("0"));
    assert!(!doc.is_modified());
}

#[test]
fn failed_step_is_not_recorded() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let err = execute(&mut doc, Command::InsertRows { at: 5000, count: 1 });
    assert!(err.is_err());
    assert!(!doc.history().can_undo());
}

#[test]
fn blank_document_takes_rows_and_columns() {
    let mut doc = Document::new();
    execute(&mut doc, Command::InsertRows { at: 0, count: 3 }).unwrap();
    assert_eq!(doc.num_rows(), 3);
    execute(
        &mut doc,
        Command::AddColumn {
            name: "a".into(),
            data_type: DataType::Int64,
            at: 0,
        },
    )
    .unwrap();
    assert_eq!((doc.num_rows(), doc.num_columns()), (3, 1));
    execute(
        &mut doc,
        Command::SetCell {
            row: 1,
            column: "a".into(),
            value: Some("7".into()),
        },
    )
    .unwrap();
    let dir = TempDir::new();
    doc.save(Some(dir.join("new.parquet"))).unwrap();
    let back = Document::open(dir.join("new.parquet"), OpenOptions::default()).unwrap();
    assert_eq!((back.num_rows(), back.num_columns()), (3, 1));
}
