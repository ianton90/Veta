//! App tests: copy, cut and paste.

use super::*;

#[test]
fn paste_and_cut_struct_fields() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let point = |app: &App, row: usize| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][1].clone()
    };
    let _ = app.update(Message::Grid(id, GridEvent::Expand { column: 1 }));

    // Into one field cell; then whole field columns, header skipped.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
    app.paste(id, "5");
    assert_eq!(point(&app, 0).as_deref(), Some("{x: 1, y: 5}"));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    app.paste(id, "point.x\tpoint.y\n10\t20\n30\t40");
    assert_eq!(point(&app, 0).as_deref(), Some("{x: 10, y: 20}"));
    assert_eq!(point(&app, 1).as_deref(), Some("{x: 30, y: 40}"));

    // Cut a field cell sets it to null; cutting a field column removes
    // the field.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 1 }));
    let _ = app.copy_selection(id, true);
    assert_eq!(point(&app, 0).as_deref(), Some("{x: null, y: 20}"));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.copy_selection(id, true);
    assert_eq!(point(&app, 1).as_deref(), Some("{y: 40}"));
}

#[test]
fn cut_and_paste() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let cell = |app: &App, row: usize, column: &str| {
        let doc = app.workbook.get(id).unwrap();
        let i = doc.schema().index_of(column).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][i].clone()
    };
    // A block pasted at a cell, as one undo step.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 1 }));
    let _ = app.update(Message::Pasted(Some("x\t1.5\r\ny\t\r\n".into())));
    assert_eq!(cell(&app, 0, "name").as_deref(), Some("x"));
    assert_eq!(cell(&app, 0, "score").as_deref(), Some("1.5"));
    assert_eq!(cell(&app, 1, "name").as_deref(), Some("y"));
    assert_eq!(cell(&app, 1, "score"), None);
    let doc = app.workbook.get(id).unwrap();
    assert!(doc.history().can_undo());

    // Cut clears the cell.
    let _ = app.update(Message::Action(Action::Cut));
    assert_eq!(cell(&app, 0, "name"), None);

    // Whole columns: a copied header row is skipped.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Pasted(Some("name\nfirst\n".into())));
    assert_eq!(cell(&app, 0, "name").as_deref(), Some("first"));

    // A bad value reports an error and changes nothing.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
    let before = app.errors.len();
    let _ = app.update(Message::Pasted(Some("abc".into())));
    assert_eq!(app.errors.len(), before + 1);
    assert_eq!(cell(&app, 0, "score").as_deref(), Some("1.5"));
}
