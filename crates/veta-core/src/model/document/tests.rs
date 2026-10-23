//! Tests for `model::document`.

use super::*;
use veta_testkit::{TempDir, fixtures};

#[test]
fn title_uses_file_name_or_untitled() {
    let mut doc = Document::new();
    assert_eq!(doc.title(), "Untitled");

    doc.path = Some(PathBuf::from("/data/sales.parquet"));
    assert_eq!(doc.title(), "sales.parquet");
}

use crate::controller::{execute, undo};
use crate::{Command, StatisticsLevel};

fn reopen(path: &Path) -> Document {
    Document::open(path, OpenOptions::default()).unwrap()
}

#[test]
fn save_as_keeps_writer_settings_and_metadata() {
    let dir = TempDir::new();
    let source = dir.join("mixed.parquet");
    fixtures::mixed_settings(&source);
    let original = reopen(&source);

    let mut doc = reopen(&source);
    let copy = dir.join("copy.parquet");
    doc.save(Some(copy.clone())).unwrap();
    assert_eq!(doc.path(), Some(copy.as_path()));
    assert!(!doc.is_modified());

    let saved = reopen(&copy);
    assert_eq!(saved.writer_settings(), original.writer_settings());
    let info = saved.file_info().unwrap();
    assert_eq!(
        info.row_groups.len(),
        original.file_info().unwrap().row_groups.len()
    );
    for (key, value) in fixtures::MIXED_SETTINGS_METADATA {
        assert_eq!(
            saved.metadata().get(key).unwrap().value.as_deref(),
            Some(*value)
        );
    }
    assert_eq!(
        saved.read(0..1000).unwrap(),
        original.read(0..1000).unwrap()
    );
    assert!(
        dir.path()
            .read_dir()
            .unwrap()
            .all(|e| { !e.unwrap().file_name().to_string_lossy().ends_with(".tmp") })
    );
}

#[test]
fn save_as_keeps_steps_and_undo() {
    let dir = TempDir::new();
    let source = dir.join("mixed.parquet");
    fixtures::mixed_settings(&source);
    let mut doc = reopen(&source);
    execute(
        &mut doc,
        Command::AddStep(Step::DeleteRows { rows: vec![0..10] }),
    )
    .unwrap();
    doc.save(Some(dir.join("out.parquet"))).unwrap();
    assert_eq!(doc.steps().len(), 1, "source untouched, steps kept");
    assert!(!doc.is_modified());
    assert_eq!(reopen(&dir.join("out.parquet")).num_rows(), 990);
    assert_eq!(reopen(&source).num_rows(), 1000);
    undo(&mut doc);
    assert!(doc.is_modified());
}

#[test]
fn save_in_place_keeps_steps_and_undo() {
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);
        let mut doc = Document::open(&path, options).unwrap();
        let rows = doc.num_rows();
        execute(
            &mut doc,
            Command::AddStep(Step::InsertRows { at: 0, count: 1 }),
        )
        .unwrap();
        execute(
            &mut doc,
            Command::SetCell {
                row: 0,
                column: "name".into(),
                value: Some("new".into()),
            },
        )
        .unwrap();
        execute(
            &mut doc,
            Command::AddStep(Step::RenameColumn {
                from: "score".into(),
                to: "points".into(),
            }),
        )
        .unwrap();
        doc.save(None).unwrap();

        // Steps and undo survive the save (#61).
        assert_eq!(doc.steps().len(), 3);
        assert!(doc.history().can_undo());
        assert!(!doc.is_modified());
        assert_eq!(doc.num_rows(), rows + 1);
        let reloaded = reopen(&path);
        assert_eq!(reloaded.num_rows(), rows + 1);
        assert!(reloaded.schema().field_with_name("points").is_ok());
        // The renamed column kept its settings (gzip, no statistics).
        let points = reloaded.writer_settings().column("points");
        assert_eq!(points.statistics, StatisticsLevel::None);
        assert!(matches!(points.compression, crate::Compression::Gzip(_)));

        // Undo past the save reads the original data, not the new file.
        crate::controller::undo(&mut doc);
        crate::controller::undo(&mut doc);
        crate::controller::undo(&mut doc);
        assert!(doc.is_modified());
        assert_eq!(doc.num_rows(), rows);
        assert!(doc.schema().field_with_name("score").is_ok());
        doc.save(None).unwrap();
        let reloaded = reopen(&path);
        assert_eq!(reloaded.num_rows(), rows);
        assert!(reloaded.schema().field_with_name("score").is_ok());
        crate::controller::redo(&mut doc).unwrap();
        assert!(doc.is_modified());
    }
}

#[test]
fn failed_save_leaves_everything_alone() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    let mut doc = reopen(&path);
    execute(
        &mut doc,
        Command::AddStep(Step::InsertRows { at: 0, count: 1 }),
    )
    .unwrap();
    let err = doc.save(Some(dir.join("missing-dir").join("x.parquet")));
    assert!(err.is_err());
    assert!(doc.is_modified());
    assert_eq!(doc.path(), Some(path.as_path()));
    assert!(Document::new().save(None).is_err(), "no path");
}

#[test]
fn open_reads_rows() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);

    let doc = Document::open(&path, OpenOptions::default()).unwrap();
    assert_eq!(doc.title(), "mixed.parquet");
    assert_eq!(doc.num_rows(), fixtures::MIXED_SETTINGS_ROWS);
    assert_eq!(doc.num_columns(), 4);
    assert_eq!(doc.read(10..20).unwrap().num_rows(), 10);
    assert!(doc.file_info().is_some());
}
