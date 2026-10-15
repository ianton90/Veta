//! Whole workflows through the public API: open a file, run commands, save,
//! reopen, in memory and paged.

#![allow(clippy::unwrap_used, clippy::single_range_in_vec_init)] // Tests.

use veta_core::controller::{execute, undo};
use veta_core::display::format_batch;
use veta_core::steps::{Condition, Filter, FilterOp, Sort, SortKey};
use veta_core::{Command, Document, OpenOptions};
use veta_testkit::{TempDir, fixtures};

fn all(doc: &Document) -> String {
    format_batch(&doc.read(0..doc.num_rows()).unwrap())
        .unwrap()
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.as_deref().unwrap_or("null"))
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn edits() -> Vec<Command> {
    vec![
        Command::SetCell {
            row: 0,
            column: "name".into(),
            value: Some("first".into()),
        },
        Command::InsertRows { at: 2, count: 3 },
        Command::DeleteRows {
            rows: vec![10..20, 15..30],
        },
        Command::AddColumn {
            name: "note".into(),
            data_type: veta_core::arrow::datatypes::DataType::Utf8,
            at: 1,
        },
        Command::RenameColumn {
            from: "score".into(),
            to: "points".into(),
        },
        Command::Filter(Filter {
            conditions: vec![Condition {
                column: "id".into(),
                op: FilterOp::Less,
                value: "900".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        }),
        Command::Sort(Sort {
            keys: vec![SortKey {
                column: "points".into(),
                descending: true,
                nulls_first: false,
            }],
        }),
        Command::SetMetadata {
            key: "owner".into(),
            value: Some("tests".into()),
        },
    ]
}

#[test]
fn edit_save_and_reopen() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    let mut outputs = Vec::new();
    for (k, options) in [OpenOptions::default(), OpenOptions::paged()]
        .into_iter()
        .enumerate()
    {
        let mut doc = Document::open(&path, options).unwrap();
        // Filter and sort run as background passes; finish each before the
        // next command, as the app does.
        for command in edits() {
            execute(&mut doc, command).unwrap();
            doc.compute_all().unwrap();
        }
        assert!(doc.is_modified());
        let expected = all(&doc);
        let out = dir.join(&format!("out-{k}.parquet"));
        doc.save(Some(out.clone())).unwrap();
        assert!(!doc.is_modified());

        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert!(saved.steps().is_empty(), "steps are applied on save");
        assert_eq!(all(&saved), expected);
        assert_eq!(saved.schema().field(1).name(), "note");
        let owner = saved
            .metadata()
            .get("owner")
            .and_then(|kv| kv.value.clone());
        assert_eq!(owner.as_deref(), Some("tests"));
        outputs.push(expected);
    }
    assert_eq!(outputs[0], outputs[1], "in memory and paged agree");
}

#[test]
fn save_in_place_keeps_undo() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    let original = all(&Document::open(&path, OpenOptions::default()).unwrap());
    let mut doc = Document::open(&path, OpenOptions::paged()).unwrap();
    execute(&mut doc, Command::DeleteRows { rows: vec![0..500] }).unwrap();
    doc.save(None).unwrap();
    let reopened = Document::open(&path, OpenOptions::default()).unwrap();
    assert_eq!(reopened.num_rows(), 500);

    // Undo after saving brings the rows back; saving again restores the file.
    assert!(undo(&mut doc));
    assert_eq!(doc.num_rows(), 1000);
    doc.save(None).unwrap();
    assert_eq!(
        all(&Document::open(&path, OpenOptions::default()).unwrap()),
        original
    );
}

#[test]
fn blank_document_saves() {
    let dir = TempDir::new();
    let mut doc = Document::new();
    execute(
        &mut doc,
        Command::AddColumn {
            name: "n".into(),
            data_type: veta_core::arrow::datatypes::DataType::Int64,
            at: 0,
        },
    )
    .unwrap();
    execute(&mut doc, Command::InsertRows { at: 0, count: 2 }).unwrap();
    execute(
        &mut doc,
        Command::SetCell {
            row: 1,
            column: "n".into(),
            value: Some("7".into()),
        },
    )
    .unwrap();
    let out = dir.join("new.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(all(&saved), "null\n7");
}
