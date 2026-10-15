use super::*;

#[test]
fn insert_and_delete_rows() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let rows = |app: &App| app.workbook.get(id).unwrap().num_rows();

    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 2,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 4,
            extend: true,
        },
    ));
    assert_eq!(app.selected_rows(id), Some(2..5));

    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Rows,
            position: iced::Point::ORIGIN,
        },
    ));
    assert!(app.menu.is_some());
    let _ = app.update(Message::Menu(MenuItem::InsertRowsBelow));
    assert!(app.menu.is_none());
    assert_eq!(rows(&app), 1003);
    assert_eq!(
        app.selected_rows(id),
        Some(5..8),
        "inserted rows are selected"
    );
    assert_eq!(app.tabs[0].grid.value(5, 0), Some(None));

    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    assert_eq!(rows(&app), 1000);
    // Like Excel, the same row positions stay selected after a delete.
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    assert_eq!(rows(&app), 997);

    let _ = app.update(Message::Action(Action::Undo));
    let _ = app.update(Message::Action(Action::Undo));
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(rows(&app), 1000);
    assert!(!app.workbook.get(id).unwrap().is_modified());
}

#[test]
fn keep_selected_rows_from_the_menu() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 2,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 4,
            extend: true,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Rows,
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::KeepRows));
    assert_eq!(app.workbook.get(id).unwrap().num_rows(), 3);
    assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("2")));
}

#[test]
fn rows_dialog_inserts_and_deletes_many() {
    use crate::dialogs::RowPlace;
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let total = fixtures::MIXED_SETTINGS_ROWS;
    let rows = |app: &App| app.workbook.get(id).unwrap().num_rows();
    let first_id = |app: &App| app.tabs[0].grid.value(0, 0).flatten().map(str::to_owned);
    let rows_msg = |app: &mut App, message| {
        let _ = app.update(Message::Rows(message));
    };

    // 3 rows at the top.
    let _ = app.update(Message::Grid(id, GridEvent::Navigate(Nav::Top)));
    let _ = app.update(Message::Action(Action::InsertRowsAt));
    rows_msg(&mut app, RowsMessage::Count("3".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::Top));
    rows_msg(&mut app, RowsMessage::Apply);
    assert!(app.dialog.is_none());
    assert_eq!(rows(&app), total + 3);
    assert_eq!(first_id(&app), None, "new rows are empty");
    assert_eq!(app.selected_rows(id), Some(0..3));

    // Delete them again: 3 rows from row 1.
    let _ = app.update(Message::Action(Action::RemoveRows));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::At));
    rows_msg(&mut app, RowsMessage::At("1".into()));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!(rows(&app), total);
    assert_eq!(first_id(&app).as_deref(), Some("0"));

    // 2 at the bottom, then delete 5 from the bottom.
    let _ = app.update(Message::Action(Action::InsertRowsAt));
    rows_msg(&mut app, RowsMessage::Count("2".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::Bottom));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!(rows(&app), total + 2);
    let _ = app.update(Message::Action(Action::RemoveRows));
    rows_msg(&mut app, RowsMessage::Count("5".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::Bottom));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!(rows(&app), total - 3);

    // Bad input keeps the dialog open.
    let _ = app.update(Message::Action(Action::RemoveRows));
    for (count, at) in [
        ("0", "1"),
        ("x", "1"),
        ("2", "0"),
        ("2", &(total - 3).to_string()),
    ] {
        rows_msg(&mut app, RowsMessage::Count(count.into()));
        rows_msg(&mut app, RowsMessage::Place(RowPlace::At));
        rows_msg(&mut app, RowsMessage::At(at.into()));
        rows_msg(&mut app, RowsMessage::Apply);
        assert!(
            matches!(app.dialog, Some(Dialog::Rows { .. })),
            "{count} at {at}"
        );
    }
    rows_msg(&mut app, RowsMessage::Cancel);
    assert!(app.dialog.is_none());
    assert_eq!(rows(&app), total - 3);

    // Insert at a row number: before that row.
    let _ = app.update(Message::Action(Action::InsertRowsAt));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::At));
    rows_msg(&mut app, RowsMessage::At("2".into()));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!(app.tabs[0].grid.value(1, 0), Some(None));
    assert_eq!(app.tabs[0].grid.value(2, 0), Some(Some("1")));

    // Without a filter, Keep/Remove Rows work by position.
    let _ = app.update(Message::Action(Action::RemoveRows));
    rows_msg(&mut app, RowsMessage::Count("1".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::At));
    rows_msg(&mut app, RowsMessage::At("2".into()));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!(rows(&app), total - 3);
    assert_eq!(app.tabs[0].grid.value(1, 0), Some(Some("1")));
    let _ = app.update(Message::Action(Action::KeepRows));
    rows_msg(&mut app, RowsMessage::Count("10".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::At));
    rows_msg(&mut app, RowsMessage::At("6".into()));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!(rows(&app), 10);
    assert_eq!(first_id(&app).as_deref(), Some("5"));
    let _ = app.update(Message::Action(Action::KeepRows));
    rows_msg(&mut app, RowsMessage::Count("3".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::Bottom));
    rows_msg(&mut app, RowsMessage::Apply);
    assert_eq!((rows(&app), first_id(&app).as_deref()), (3, Some("12")));
    let _ = app.update(Message::Action(Action::KeepRows));
    rows_msg(&mut app, RowsMessage::Count("4".into()));
    rows_msg(&mut app, RowsMessage::Place(RowPlace::Top));
    rows_msg(&mut app, RowsMessage::Apply);
    assert!(
        matches!(app.dialog, Some(Dialog::Rows { .. })),
        "only 3 rows"
    );
}

#[test]
fn rows_before_columns_in_a_new_file() {
    let mut app = app();
    let _ = app.update(Message::Action(Action::New));
    let id = app.active.unwrap();
    app.menu = Some(ContextMenu {
        document: id,
        target: MenuTarget::Blank,
        position: iced::Point::ORIGIN,
    });
    let _ = app.update(Message::Menu(MenuItem::AddRow));
    let _ = app.update(Message::Grid(id, GridEvent::InsertRows));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    let _ = app.view();
    assert_eq!(app.workbook.get(id).unwrap().num_rows(), 1);
}
