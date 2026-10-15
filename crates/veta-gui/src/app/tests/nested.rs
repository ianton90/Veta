//! App tests: struct fields, list items and nested columns.

use super::*;

#[test]
fn nested_values_in_formula_bar() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 1 }));
    assert_eq!(app.tabs[0].grid.value(0, 1), Some(Some("{x: 1, y: 2}")));
    assert_eq!(
        app.tabs[0].grid.edit_value(0, 1),
        Some(Some(r#"{"x":1,"y":2}"#))
    );
    assert_eq!(app.cell_text(id).as_deref(), Some(r#"{"x":1,"y":2}"#));
}

#[test]
fn struct_columns_expand_into_editable_fields() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let grid = |app: &mut App, event| {
        let _ = app.update(Message::Grid(id, event));
    };
    let cell = |app: &App, row: usize| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][1].clone()
    };

    grid(&mut app, GridEvent::Expand { column: 1 });
    let g = &app.tabs[0].grid;
    assert_eq!(g.value(0, 1), Some(Some("1")));
    assert_eq!(g.value(0, 2), Some(Some("2")));
    assert_eq!(g.value(2, 1), Some(None), "null struct, null fields");
    assert_eq!(g.value(0, 3), Some(Some("[2 items]")), "tags moved right");

    // A field cell edits that field of the struct.
    grid(&mut app, GridEvent::Select { row: 0, column: 2 });
    assert_eq!(app.tabs[0].grid.selected(), Some((0, 1)));
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(app.edit.as_ref().unwrap().text, "2");
    let _ = app.update(Message::EditInput("20".into()));
    let _ = app.update(Message::CommitEdit);
    assert_eq!(cell(&app, 0).as_deref(), Some("{x: 1, y: 20}"));
    grid(&mut app, GridEvent::Select { row: 2, column: 2 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    let _ = app.update(Message::EditInput("5".into()));
    let _ = app.update(Message::CommitEdit);
    assert_eq!(cell(&app, 2).as_deref(), Some("{x: null, y: 5}"));
    grid(&mut app, GridEvent::Select { row: 2, column: 2 });
    grid(&mut app, GridEvent::ClearCell);
    assert_eq!(cell(&app, 2).as_deref(), Some("{x: null, y: null}"));

    // Column commands see document columns.
    grid(
        &mut app,
        GridEvent::SelectColumns {
            column: 2,
            extend: false,
        },
    );
    assert_eq!(app.tabs[0].grid.selected_columns(), Some(1..2));
    app.tabs[0].grid.select_columns(2..3);
    assert_eq!(app.tabs[0].grid.selection(), Selection::Columns(2..3));

    // Nested structs expand level by level; collapsing restores.
    grid(&mut app, GridEvent::Expand { column: 6 });
    grid(&mut app, GridEvent::Expand { column: 7 });
    let g = &app.tabs[0].grid;
    assert_eq!(g.value(0, 6), Some(Some("Ana")));
    assert_eq!(g.value(0, 8), Some(Some("6")), "owner.home.y");
    grid(&mut app, GridEvent::Collapse { column: 7 });
    grid(&mut app, GridEvent::Collapse { column: 1 });
    let g = &app.tabs[0].grid;
    assert_eq!(g.value(0, 1), Some(Some("{x: 1, y: 20}")));
    assert_eq!(g.value(0, 5), Some(Some("Ana")), "owner still expanded");
}

#[test]
fn struct_fields_edit_in_place() {
    use crate::dialogs::{TypeChoice, TypeOption};
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let grid = |app: &mut App, event| {
        let _ = app.update(Message::Grid(id, event));
    };
    let enter = |app: &mut App, text: &str| {
        let _ = app.update(Message::EditInput(text.into()));
        let _ = app.update(Message::CommitEdit);
    };
    let cell = |app: &App, row: usize, column: &str| {
        let doc = app.workbook.get(id).unwrap();
        let i = doc.schema().index_of(column).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][i].clone()
    };
    let menu = |app: &mut App, target, item| {
        app.menu = Some(ContextMenu {
            document: id,
            target,
            position: iced::Point::ORIGIN,
        });
        let _ = app.update(Message::Menu(item));
    };

    // Enter on a struct cell edits its first field; Enter moves on.
    grid(&mut app, GridEvent::Select { row: 0, column: 1 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(app.edit.as_ref().unwrap().text, "1");
    enter(&mut app, "10");
    assert_eq!(cell(&app, 0, "point").as_deref(), Some("{x: 10, y: 2}"));
    assert_eq!(
        app.selected_list_item(id).unwrap().field.as_deref(),
        Some("y")
    );
    grid(&mut app, GridEvent::ClearCell);
    assert_eq!(cell(&app, 0, "point").as_deref(), Some("{x: 10, y: null}"));

    // A null struct gets the field set.
    grid(&mut app, GridEvent::Select { row: 2, column: 1 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    enter(&mut app, "4");
    assert_eq!(cell(&app, 2, "point").as_deref(), Some("{x: 4, y: null}"));

    // Nested fields edit as JSON.
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 5,
            item: 1,
        },
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(app.edit.as_ref().unwrap().text, r#"{"x":5,"y":6}"#);
    enter(&mut app, r#"{"x": 7}"#);
    assert_eq!(
        cell(&app, 0, "owner").as_deref(),
        Some("{name: Ana, home: {x: 7, y: null}}")
    );

    // Add a field from the field menu, then remove one.
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 1,
            item: 0,
        },
    );
    menu(&mut app, MenuTarget::Field, MenuItem::AddField);
    let column = |app: &mut App, message| {
        let _ = app.update(Message::ColumnDialog(message));
    };
    column(&mut app, ColumnMessage::Name("z".into()));
    column(
        &mut app,
        ColumnMessage::Type(TypeOption::Plain(TypeChoice::Integer)),
    );
    column(&mut app, ColumnMessage::Submit);
    assert!(app.dialog.is_none());
    assert_eq!(
        cell(&app, 0, "point").as_deref(),
        Some("{x: 10, y: null, z: null}")
    );
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 1,
            item: 1,
        },
    );
    menu(&mut app, MenuTarget::Field, MenuItem::RemoveField);
    assert_eq!(cell(&app, 0, "point").as_deref(), Some("{x: 10, z: null}"));
}

#[test]
fn list_items_edit_in_place() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let grid = |app: &mut App, event| {
        let _ = app.update(Message::Grid(id, event));
    };
    let enter = |app: &mut App, text: &str| {
        let _ = app.update(Message::EditInput(text.into()));
        let _ = app.update(Message::CommitEdit);
    };
    let tags = |app: &App, row: usize| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][2].clone()
    };
    let editing = |app: &App| {
        app.edit
            .as_ref()
            .and_then(|e| e.item.as_ref())
            .map(|i| i.index)
    };

    // An empty list: Enter edits the "empty" slot; each value goes on to
    // a new item at the end, an empty one ends.
    grid(&mut app, GridEvent::Select { row: 1, column: 2 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(editing(&app), Some(0));
    enter(&mut app, "x");
    assert_eq!(tags(&app, 1).as_deref(), Some("[x]"));
    assert_eq!(editing(&app), Some(1), "goes on at the end");
    enter(&mut app, "y");
    enter(&mut app, "");
    assert!(app.edit.is_none());
    assert_eq!(tags(&app, 1).as_deref(), Some("[x, y]"));

    // Editing the last item also goes on at the end.
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 2,
            item: 1,
        },
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(app.edit.as_ref().unwrap().text, "b");
    enter(&mut app, "B");
    assert_eq!(editing(&app), Some(2));
    enter(&mut app, "");
    assert_eq!(tags(&app, 0).as_deref(), Some("[a, B]"));

    // Insert a null in the middle, delete it, set an item to null.
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 2,
            item: 1,
        },
    );
    grid(&mut app, GridEvent::InsertRows);
    assert_eq!(tags(&app, 0).as_deref(), Some("[a, null, B]"));
    app.menu = Some(ContextMenu {
        document: id,
        target: MenuTarget::Item,
        position: iced::Point::ORIGIN,
    });
    let _ = app.update(Message::Menu(MenuItem::InsertItemBelow));
    assert_eq!(tags(&app, 0).as_deref(), Some("[a, null, null, B]"));
    assert_eq!(app.selected_list_item(id).map(|i| i.index), Some(2));
    grid(&mut app, GridEvent::DeleteRows);
    assert_eq!(tags(&app, 0).as_deref(), Some("[a, null, B]"));
    grid(&mut app, GridEvent::Navigate(Nav::Up));
    grid(&mut app, GridEvent::DeleteRows);
    assert_eq!(tags(&app, 0).as_deref(), Some("[a, B]"));
    grid(&mut app, GridEvent::Navigate(Nav::Up));
    grid(&mut app, GridEvent::ClearCell);
    assert_eq!(tags(&app, 0).as_deref(), Some("[null, B]"));

    // Typing on a list cell adds an item at the end; a null list too.
    grid(&mut app, GridEvent::Select { row: 3, column: 2 });
    grid(
        &mut app,
        GridEvent::StartEdit {
            initial: Some("z".into()),
        },
    );
    let _ = app.update(Message::CommitEdit);
    assert_eq!(tags(&app, 3).as_deref(), Some("[c, null, d, z]"));
    let _ = app.update(Message::CommitEdit);
    grid(&mut app, GridEvent::Select { row: 2, column: 2 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    enter(&mut app, "q");
    assert_eq!(tags(&app, 2).as_deref(), Some("[q]"));
}

#[test]
fn edit_nested_values_and_expand_explode() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let cell = |app: &App, row: usize, column: &str| {
        let doc = app.workbook.get(id).unwrap();
        let i = doc.schema().index_of(column).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][i].clone()
    };

    // Lists edit item by item; other nested cells as JSON.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 1, column: 2 }));
    let _ = app.update(Message::Grid(id, GridEvent::StartEdit { initial: None }));
    for text in ["x", "y", ""] {
        let _ = app.update(Message::EditInput(text.into()));
        let _ = app.update(Message::CommitEdit);
    }
    assert_eq!(cell(&app, 1, "tags").as_deref(), Some("[x, y]"));
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 1, column: 4 }));
    let _ = app.update(Message::Grid(id, GridEvent::StartEdit { initial: None }));
    assert_eq!(app.edit.as_ref().unwrap().text, "{}");
    let _ = app.update(Message::EditInput(r#"{"a": 1}"#.into()));
    let _ = app.update(Message::CommitEdit);
    assert_eq!(cell(&app, 1, "scores").as_deref(), Some("{a: 1}"));

    // Column menu: expand the struct, explode the list.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Column(1),
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::ExpandStruct));
    assert_eq!(cell(&app, 0, "point.x").as_deref(), Some("1"));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 3,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::ExplodeList));
    finish_compute(&mut app, id);
    // Row 1 now has two tags: 2 + 2 + 1 (null row) + 3.
    assert_eq!(app.workbook.get(id).unwrap().num_rows(), 8);
}
