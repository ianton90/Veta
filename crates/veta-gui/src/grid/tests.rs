use super::*;
use veta_core::OpenOptions;
use veta_testkit::{TempDir, fixtures};

fn open_large(dir: &TempDir, rows: usize) -> Document {
    let path = dir.join("large.parquet");
    fixtures::large(&path, rows, 50_000);
    Document::open(&path, OpenOptions::paged()).unwrap()
}

fn resized(view: &mut GridView, doc: &Document, rows: usize) {
    view.apply(GridEvent::Resized { rows, width: 500.0 }, doc);
}

#[test]
fn loads_only_a_window_around_the_view() {
    let dir = TempDir::new();
    let doc = open_large(&dir, 200_000);
    let mut view = GridView::new(&doc, 13.0);
    resized(&mut view, &doc, 20);

    view.apply(
        GridEvent::Scroll {
            first_row: 150_000,
            scroll_x: 0.0,
        },
        &doc,
    );
    assert!(view.cells.len() <= 3 * 20 + 1);
    assert_eq!(
        view.cell(150_000, 0),
        Some(&Some("150000".to_owned())),
        "first visible row is loaded"
    );
    assert_eq!(view.cell(150_020, 2), Some(&Some("row-150020".to_owned())));
}

#[test]
fn list_cells_open_into_item_lines() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let doc = Document::open(&path, OpenOptions::default()).unwrap();
    let mut view = GridView::new(&doc, 13.0);
    resized(&mut view, &doc, 4);
    assert_eq!(view.value(0, 2), Some(Some("[2 items]")));
    assert_eq!(view.value(0, 4), Some(Some("{2 entries}")));
    assert!(view.has_items(3, 2) && !view.has_items(0, 0));
    assert!(view.has_items(1, 2), "empty lists open, to add items");
    assert!(!view.has_items(1, 4), "empty maps don't");

    view.apply(GridEvent::ToggleItems { row: 3 }, &doc);
    // Row 3: tags has 3 items, scores 1 entry.
    assert_eq!(view.row_lines(3), 4);
    assert_eq!(view.row_at_line(0), Some((view.first_row, 0)));
    assert_eq!(view.first_row, 3, "scrolled so the open row fits");
    assert_eq!(view.max_first_row(), 3);
    view.apply(
        GridEvent::Scroll {
            first_row: 0,
            scroll_x: 0.0,
        },
        &doc,
    );
    assert_eq!(view.row_at_line(3), Some((3, 0)));
    assert_eq!(view.row_at_line(5), Some((3, 2)));
    assert_eq!(view.rows_fitting(0), 3);
    view.apply(GridEvent::ToggleItems { row: 3 }, &doc);
    assert_eq!(view.row_lines(3), 1);
    assert_eq!(view.max_first_row(), 0);
}

#[test]
fn background_reads_while_scrolling() {
    let dir = TempDir::new();
    let doc = open_large(&dir, 200_000);
    let mut view = GridView::new(&doc, 13.0);
    view.set_background(true);
    resized(&mut view, &doc, 20);
    let scroll = |view: &mut GridView, first_row| {
        view.apply(
            GridEvent::Scroll {
                first_row,
                scroll_x: 0.0,
            },
            &doc,
        );
    };

    scroll(&mut view, 100_000);
    assert_eq!(view.cell(100_000, 0), None, "not read yet");
    let first = view.take_read().expect("a read");
    assert!(view.take_read().is_none(), "taken once");
    scroll(&mut view, 100_005);
    assert!(view.take_read().is_none(), "the read on its way covers it");
    scroll(&mut view, 150_000);
    let second = view.take_read().expect("a new read");

    // The older read arrives late and is dropped.
    view.install_read(first.id, first.run(), &doc);
    assert_eq!(view.cell(150_000, 0), None);
    view.install_read(second.id, second.run(), &doc);
    assert_eq!(view.cell(150_000, 0), Some(&Some("150000".to_owned())));
    assert!(view.take_read().is_none(), "nothing more needed");
    // A failed read shows as an error.
    scroll(&mut view, 10);
    let third = view.take_read().unwrap();
    view.install_read(third.id, Err("disk gone".into()), &doc);
    assert_eq!(view.error(), Some("Disk gone"));
}

#[test]
fn scroll_is_clamped() {
    let dir = TempDir::new();
    let doc = open_large(&dir, 100);
    let mut view = GridView::new(&doc, 13.0);
    resized(&mut view, &doc, 30);
    view.apply(
        GridEvent::Scroll {
            first_row: 10_000,
            scroll_x: 1e9,
        },
        &doc,
    );
    assert_eq!(view.first_row, 70);
    assert!(view.scroll_x <= view.max_scroll_x());
}

#[test]
fn navigation_moves_selection_and_reveals_it() {
    let dir = TempDir::new();
    let doc = open_large(&dir, 1_000);
    let mut view = GridView::new(&doc, 13.0);
    resized(&mut view, &doc, 10);

    view.apply(GridEvent::Select { row: 9, column: 0 }, &doc);
    assert_eq!(view.first_row, 0);
    view.apply(GridEvent::Navigate(Nav::Down), &doc);
    assert_eq!(view.selected(), Some((10, 0)));
    assert_eq!(view.first_row, 1, "scrolled one row to reveal");

    view.apply(GridEvent::Navigate(Nav::Bottom), &doc);
    assert_eq!(view.selected(), Some((999, 0)));
    assert_eq!(view.first_row, 990);

    view.apply(GridEvent::Navigate(Nav::RowEnd), &doc);
    assert_eq!(view.selected(), Some((999, 2)));

    view.apply(GridEvent::Navigate(Nav::Top), &doc);
    assert_eq!(view.selected(), Some((0, 2)));
    assert_eq!(view.first_row, 0);
}

#[test]
fn column_resize_has_minimum() {
    let dir = TempDir::new();
    let doc = open_large(&dir, 10);
    let mut view = GridView::new(&doc, 13.0);
    view.apply(
        GridEvent::ColumnResized {
            column: 1,
            width: 5.0,
        },
        &doc,
    );
    assert_eq!(view.columns[1].width, MIN_COLUMN_WIDTH);
}

#[test]
fn thumb_math_round_trips() {
    let (pos, len) = thumb(400.0, 1000.0, 100.0, 450.0).unwrap();
    let offset = offset_for_thumb(400.0, len, 1000.0, 100.0, pos);
    assert!((offset - 450.0).abs() < 1e-3);
    assert!(thumb(400.0, 50.0, 100.0, 0.0).is_none());
}
