//! App tests: editing cells and undo.

use super::*;

#[test]
fn edit_commit_and_undo() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let score = |app: &App| app.tabs[0].grid.value(2, 2).flatten().map(str::to_owned);

    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 2, column: 2 }));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::StartEdit {
            initial: Some("7".into()),
        },
    ));
    let _ = app.update(Message::EditInput("7.25".into()));
    let _ = app.update(Message::CommitEdit);
    assert!(app.edit.is_none());
    assert_eq!(score(&app).as_deref(), Some("7.25"));
    assert_eq!(app.tabs[0].grid.selected(), Some((3, 2)), "moved down");
    assert!(app.workbook.get(id).unwrap().is_modified());

    // Invalid input keeps the edit open and reports the error.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::StartEdit {
            initial: Some("x".into()),
        },
    ));
    let _ = app.update(Message::CommitEdit);
    assert!(app.edit.as_ref().unwrap().error.is_some());
    assert!(
        app.errors.is_empty(),
        "shown next to the editor, not as a banner"
    );
    // The editor flashes, then fades.
    let (start, level) = app.flash.unwrap();
    assert_eq!(level, 1.0);
    let _ = app.update(Message::Frame(
        start + std::time::Duration::from_millis(250),
    ));
    let level = app.flash.unwrap().1;
    assert!(level > 0.0 && level < 1.0, "{level}");
    let _ = app.update(Message::Frame(
        start + std::time::Duration::from_millis(600),
    ));
    assert!(app.flash.is_none());
    let _ = app.update(Message::Escape);
    assert!(app.edit.is_none());

    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(score(&app).as_deref(), Some("1.0"));
    let _ = app.update(Message::Action(Action::Redo));
    assert_eq!(score(&app).as_deref(), Some("7.25"));
}
