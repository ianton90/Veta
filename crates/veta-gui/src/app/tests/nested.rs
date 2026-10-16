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
    // Nested cells show as text but aren't typed as JSON: they open.
    assert_eq!(app.cell_text(id).as_deref(), Some("{x: 1, y: 2}"));
    assert!(app.tabs[0].grid.selected_is_container());
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 4 }));
    assert_eq!(app.cell_text(id).as_deref(), Some("{2 entries}"));
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
    assert_eq!(app.selected_cell_item(id).unwrap().item.field(), Some("y"));
    grid(&mut app, GridEvent::ClearCell);
    assert_eq!(cell(&app, 0, "point").as_deref(), Some("{x: 10, y: null}"));

    // A null struct gets the field set.
    grid(&mut app, GridEvent::Select { row: 2, column: 1 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    enter(&mut app, "4");
    assert_eq!(cell(&app, 2, "point").as_deref(), Some("{x: 4, y: null}"));

    // A struct inside a struct opens: Enter goes to its first field.
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 5,
            item: 1,
        },
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert!(app.edit.is_none(), "structs aren't typed as text");
    let item = app.selected_cell_item(id).unwrap();
    assert_eq!(item.item.display, ".home.x");
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(app.edit.as_ref().unwrap().text, "5");
    enter(&mut app, "7");
    assert_eq!(
        cell(&app, 0, "owner").as_deref(),
        Some("{name: Ana, home: {x: 7, y: 6}}")
    );
    // Space closes it again.
    grid(&mut app, GridEvent::Navigate(Nav::Up));
    grid(&mut app, GridEvent::Navigate(Nav::Up));
    assert_eq!(app.selected_cell_item(id).unwrap().item.display, ".home");
    grid(&mut app, GridEvent::ToggleNode);
    grid(&mut app, GridEvent::Navigate(Nav::Down));
    assert!(app.selected_cell_item(id).is_none(), "past the last line");

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
        ColumnMessage::Type(vec![], TypeOption::Plain(TypeChoice::Integer)),
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
            .map(|i| i.item.index)
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
    assert_eq!(app.selected_cell_item(id).map(|i| i.item.index), Some(2));
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
fn map_entries_edit_in_place() {
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
    let scores = |app: &App, row: usize| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][4].clone()
    };
    let editing = |app: &App| {
        app.edit
            .as_ref()
            .map(|e| (e.item.as_ref().unwrap().item.index, e.key))
    };
    let menu = |app: &mut App, item| {
        app.menu = Some(ContextMenu {
            document: id,
            target: MenuTarget::Entry,
            position: iced::Point::ORIGIN,
        });
        let _ = app.update(Message::Menu(item));
    };

    // An empty map: Enter edits the new key, then its value, then the next
    // key; an empty key ends.
    grid(&mut app, GridEvent::Select { row: 1, column: 4 });
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(editing(&app), Some((0, true)));
    enter(&mut app, "math");
    assert_eq!(editing(&app), Some((0, false)), "then its value");
    assert_eq!(scores(&app, 1).as_deref(), Some("{math: null}"));
    enter(&mut app, "4");
    assert_eq!(editing(&app), Some((1, true)), "then the next key");
    enter(&mut app, "art");
    enter(&mut app, "5");
    enter(&mut app, "");
    assert!(app.edit.is_none());
    assert_eq!(scores(&app, 1).as_deref(), Some("{math: 4, art: 5}"));

    // Edit a value, rename a key; a taken key is refused.
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 4,
            item: 0,
        },
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(app.edit.as_ref().unwrap().text, "9");
    enter(&mut app, "10");
    assert!(app.edit.is_none(), "moves down to the next entry");
    assert_eq!(scores(&app, 0).as_deref(), Some("{math: 10, art: 7}"));
    grid(&mut app, GridEvent::Navigate(Nav::Up));
    menu(&mut app, MenuItem::EditKey);
    assert_eq!(app.edit.as_ref().unwrap().text, "math");
    enter(&mut app, "art");
    assert!(app.edit.as_ref().unwrap().error.is_some());
    enter(&mut app, "maths");
    assert!(app.edit.is_none());
    assert_eq!(scores(&app, 0).as_deref(), Some("{maths: 10, art: 7}"));

    // Add an entry from the menu, set a value to null, delete an entry.
    menu(&mut app, MenuItem::AddEntry);
    assert_eq!(editing(&app), Some((2, true)));
    enter(&mut app, "pe");
    enter(&mut app, "3");
    let _ = app.update(Message::Escape);
    assert_eq!(
        scores(&app, 0).as_deref(),
        Some("{maths: 10, art: 7, pe: 3}")
    );
    grid(
        &mut app,
        GridEvent::SelectItem {
            row: 0,
            column: 4,
            item: 1,
        },
    );
    grid(&mut app, GridEvent::ClearCell);
    assert_eq!(
        scores(&app, 0).as_deref(),
        Some("{maths: 10, art: null, pe: 3}")
    );
    grid(&mut app, GridEvent::DeleteRows);
    assert_eq!(scores(&app, 0).as_deref(), Some("{maths: 10, pe: 3}"));

    // Typing on a null map cell starts a new key.
    grid(&mut app, GridEvent::Select { row: 2, column: 4 });
    grid(
        &mut app,
        GridEvent::StartEdit {
            initial: Some("k".into()),
        },
    );
    assert_eq!(editing(&app), Some((0, true)));
    let _ = app.update(Message::CommitEdit);
    enter(&mut app, "1");
    assert_eq!(scores(&app, 2).as_deref(), Some("{k: 1}"));
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

    // Lists edit item by item (maps too, see `map_entries_edit_in_place`).
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 1, column: 2 }));
    let _ = app.update(Message::Grid(id, GridEvent::StartEdit { initial: None }));
    for text in ["x", "y", ""] {
        let _ = app.update(Message::EditInput(text.into()));
        let _ = app.update(Message::CommitEdit);
    }
    assert_eq!(cell(&app, 1, "tags").as_deref(), Some("[x, y]"));

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

#[test]
fn values_nested_to_any_depth_open_and_edit() {
    let dir = TempDir::new();
    let path = dir.join("deep.parquet");
    fixtures::deep_nested(&path, 40);
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
    let at = |app: &App| app.selected_cell_item(id).map(|i| i.item.display);
    let column = |name: &str| {
        veta_testkit::fixtures::deep_nested_schema()
            .index_of(name)
            .unwrap()
    };
    let find = |app: &App, name: &str, test: &dyn Fn(&str) -> bool| {
        (0..40)
            .find(|&r| cell(app, r, name).is_some_and(|v| test(&v)))
            .unwrap()
    };

    // A list of lists, from empty: the new item is a list, so it opens.
    let row = find(&app, "matrix", &|v| v == "[]");
    grid(
        &mut app,
        GridEvent::Select {
            row,
            column: column("matrix"),
        },
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(cell(&app, row, "matrix").as_deref(), Some("[null]"));
    assert_eq!(at(&app).as_deref(), Some("[0][0]"), "the inner list's slot");
    grid(&mut app, GridEvent::StartEdit { initial: None });
    enter(&mut app, "1");
    enter(&mut app, "2");
    enter(&mut app, "");
    assert_eq!(cell(&app, row, "matrix").as_deref(), Some("[[1, 2]]"));
    assert_eq!(at(&app).as_deref(), Some("[0]"), "back on the inner list");

    // A map inside a struct inside a list: rename a key, set its value.
    let row = find(&app, "orders", &|v| {
        v.starts_with("[{") && v.contains("color")
    });
    grid(
        &mut app,
        GridEvent::Select {
            row,
            column: column("orders"),
        },
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    assert_eq!(at(&app).as_deref(), Some("[0].sku"), "into the first order");
    for _ in 0..3 {
        grid(&mut app, GridEvent::Navigate(Nav::Down));
    }
    assert_eq!(at(&app).as_deref(), Some("[0].options"));
    grid(&mut app, GridEvent::ToggleNode);
    grid(&mut app, GridEvent::Navigate(Nav::Down));
    assert_eq!(at(&app).as_deref(), Some(r#"[0].options["color"]"#));
    app.menu = Some(ContextMenu {
        document: id,
        target: MenuTarget::Entry,
        position: iced::Point::ORIGIN,
    });
    let _ = app.update(Message::Menu(MenuItem::EditKey));
    enter(&mut app, "colour");
    grid(&mut app, GridEvent::StartEdit { initial: None });
    enter(&mut app, "teal");
    let orders = cell(&app, row, "orders").unwrap();
    assert!(orders.contains("options: {colour: teal, "), "{orders}");

    // A whole-number-keyed map of structs: a new entry opens its struct.
    let row = find(&app, "history", &|v| v == "{}");
    grid(
        &mut app,
        GridEvent::Select {
            row,
            column: column("history"),
        },
    );
    grid(
        &mut app,
        GridEvent::StartEdit {
            initial: Some("5".into()),
        },
    );
    let _ = app.update(Message::CommitEdit);
    assert_eq!(
        at(&app).as_deref(),
        Some("[\"5\"].status"),
        "into its struct"
    );
    grid(&mut app, GridEvent::StartEdit { initial: None });
    enter(&mut app, "paid");
    assert_eq!(
        cell(&app, row, "history").as_deref(),
        Some("{5: {status: paid, notes: null}}")
    );
}
