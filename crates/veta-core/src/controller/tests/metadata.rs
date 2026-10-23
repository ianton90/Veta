//! Controller tests: metadata and writer settings.

use super::*;

#[test]
fn set_metadata_adds_then_replaces() {
    let mut doc = Document::new();
    execute(&mut doc, set("owner", "a")).unwrap();
    execute(&mut doc, set("owner", "b")).unwrap();

    let entries = &doc.metadata().key_value;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].value.as_deref(), Some("b"));
}

#[test]
fn replace_metadata() {
    let mut doc = Document::new();
    execute(&mut doc, set("a", "1")).unwrap();
    let kv = |k: &str, v: Option<&str>| KeyValue {
        key: k.into(),
        value: v.map(str::to_owned),
    };
    let dup = vec![kv("x", None), kv("x", Some("2"))];
    assert!(execute(&mut doc, Command::ReplaceMetadata(dup)).is_err());
    assert!(execute(&mut doc, Command::ReplaceMetadata(vec![kv(" ", None)])).is_err());
    execute(
        &mut doc,
        Command::ReplaceMetadata(vec![kv("b", Some("2")), kv("c", None)]),
    )
    .unwrap();
    assert_eq!(doc.metadata().key_value.len(), 2);
    assert_eq!(value(&doc, "a"), None);
    undo(&mut doc);
    assert_eq!(value(&doc, "a"), Some("1"));
}

#[test]
fn rename_carries_writer_settings() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let zstd = doc.writer_settings().column("id").compression;
    execute(
        &mut doc,
        Command::AddStep(Step::RenameColumn {
            from: "id".into(),
            to: "key".into(),
        }),
    )
    .unwrap();
    assert_eq!(doc.writer_settings().column("key").compression, zstd);
    undo(&mut doc);
    assert_eq!(doc.writer_settings().column("id").compression, zstd);
    assert!(
        doc.writer_settings()
            .columns
            .iter()
            .all(|(p, _)| p != "key")
    );
}

#[test]
fn writer_settings_command() {
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let mut settings = doc.writer_settings().clone();
    settings.max_row_group_rows = 0;
    assert!(execute(&mut doc, Command::SetWriterSettings(settings.clone())).is_err());
    settings.max_row_group_rows = 50;
    execute(&mut doc, Command::SetWriterSettings(settings)).unwrap();
    assert_eq!(doc.writer_settings().max_row_group_rows, 50);
    undo(&mut doc);
    assert_eq!(
        doc.writer_settings().max_row_group_rows,
        fixtures::MIXED_SETTINGS_ROW_GROUP
    );
}

#[test]
fn column_metadata_is_saved_and_follows_renames() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
    let entries = vec![KeyValue {
        key: "unit".into(),
        value: Some("m".into()),
    }];
    let set = |column: &str| Command::ReplaceColumnMetadata {
        column: column.into(),
        entries: entries.clone(),
    };
    assert!(execute(&mut doc, set("missing")).is_err());
    execute(&mut doc, set("score")).unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::RenameColumn {
            from: "score".into(),
            to: "points".into(),
        }),
    )
    .unwrap();
    assert_eq!(doc.metadata().column("points"), Some(entries.as_slice()));
    let out = dir.join("out.parquet");
    doc.save(Some(out.clone())).unwrap();
    let back = Document::open(&out, OpenOptions::default()).unwrap();
    let schema = back.schema();
    let field = schema.field_with_name("points").unwrap();
    assert_eq!(field.metadata().get("unit").map(String::as_str), Some("m"));
    assert!(schema.field_with_name("id").unwrap().metadata().is_empty());
    assert!(undo(&mut doc));
    assert_eq!(doc.metadata().column("score"), Some(entries.as_slice()));
}
