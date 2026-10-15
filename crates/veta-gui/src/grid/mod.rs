//! Virtualized data grid.
//!
//! [`GridView`] is the per-tab view state (scroll position, selection, column
//! widths and the window of formatted rows currently on screen). [`Grid`] is
//! the widget that draws a `GridView` and turns mouse/keyboard input into
//! [`Event`]s, which the app feeds back into [`GridView::apply`]. Only the
//! rows in view (plus a margin) are ever read from the document.
//!
//! Files: `geometry` (columns on screen, row lines, hit testing), `items`
//! (open list and struct cells), `load` (reading rows, the filter view),
//! `input` (the widget and its event handling) and `draw`.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Renderer as _, Text};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Renderer as _, Shell};
use iced::alignment::Vertical;
use iced::keyboard::{self, key::Named};
use iced::mouse::{self, ScrollDelta};
use iced::{
    Border, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Renderer, Size, Theme,
    font,
};
use std::collections::BTreeSet;
use std::ops::Range;
use std::time::{Duration, Instant};
use veta_core::Document;
use veta_core::arrow::array::Array;
use veta_core::arrow::datatypes::{DataType, Field, Schema};
use veta_core::arrow::record_batch::{RecordBatch, RecordBatchOptions};
use veta_core::steps::{ComputeJob, Computed, Filter, Pipeline, Status};

use crate::platform::icon::{self, Icon};
use crate::settings::theme::Tokens;
use veta_core::display::{format_batch, has_items, is_numeric, type_name};

mod draw;
mod geometry;
mod input;
mod items;
mod load;
#[cfg(test)]
mod tests;

use geometry::*;
pub use input::*;
use items::*;
pub use load::*;

const SCROLLBAR: f32 = 12.0;
const MIN_THUMB: f32 = 24.0;
const CELL_PADDING: f32 = 6.0;
const MIN_COLUMN_WIDTH: f32 = 40.0;
const MIN_FITTED_WIDTH: f32 = 60.0;
const MAX_FITTED_WIDTH: f32 = 360.0;
const RESIZE_GRAB: f32 = 4.0;
const WHEEL_LINES: f32 = 3.0;
const NULL_TEXT: &str = "null";
/// Items of an open list cell shown before "… more".
const MAX_ITEMS: usize = 100;

/// Sizes derived from the text size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub text_size: f32,
    type_text_size: f32,
    row_height: f32,
    header_height: f32,
    /// Approximate width of one character, for sizing columns.
    char_width: f32,
}

impl Metrics {
    pub fn new(text_size: f32) -> Self {
        let row_height = (text_size * 1.85).round();
        Self {
            text_size,
            type_text_size: (text_size * 0.85).round(),
            row_height,
            header_height: (row_height * 1.7).round(),
            char_width: text_size * 0.62,
        }
    }
}

/// Input from the grid widget.
#[derive(Debug, Clone, PartialEq)]
pub enum GridEvent {
    /// The widget's body can show `rows` full rows and is `width` wide.
    Resized {
        rows: usize,
        width: f32,
    },
    /// Scroll to an absolute position.
    Scroll {
        first_row: usize,
        scroll_x: f32,
    },
    /// A cell was clicked.
    Select {
        row: usize,
        column: usize,
    },
    /// Keyboard navigation.
    Navigate(Nav),
    ColumnResized {
        column: usize,
        width: f32,
    },
    /// Start editing the selected cell, optionally replacing its text with
    /// what was typed.
    StartEdit {
        initial: Option<String>,
    },
    /// Set the selected cell to null.
    ClearCell,
    /// A row number was clicked; `extend` (Shift) extends the selection.
    SelectRows {
        row: usize,
        extend: bool,
    },
    /// Insert as many rows as are selected, above the selection.
    InsertRows,
    DeleteRows,
    /// A column header was clicked; `extend` (Shift) extends the selection.
    SelectColumns {
        column: usize,
        extend: bool,
    },
    /// A column header was dragged to a new position (final index).
    MoveColumn {
        from: usize,
        to: usize,
    },
    /// Double-click on a column's right edge: fit the column to its content.
    AutoFit {
        column: usize,
    },
    /// Arrow up/down or Tab while editing a cell: apply the edit and move.
    EditMove(Nav),
    /// Show the fields of the struct column as sub-columns (view only).
    Expand {
        column: usize,
    },
    /// Fold the sub-columns starting at `column` back into their struct.
    Collapse {
        column: usize,
    },
    /// Show or hide the items of the list and map cells of `row`.
    ToggleItems {
        row: usize,
    },
    /// An item of an open list cell was clicked (`item` may be the list's
    /// length: the slot for a new item).
    SelectItem {
        row: usize,
        column: usize,
        item: usize,
    },
    /// Right-click; `position` is in window coordinates.
    ContextMenu {
        target: MenuTarget,
        position: Point,
    },
}

/// What a context menu was opened on.
/// The kind of selection in a grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    None,
    Cell(usize, usize),
    Rows(Range<usize>),
    Columns(Range<usize>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    Cell,
    /// An item of an open list cell.
    Item,
    /// A field of an open struct cell.
    Field,
    /// The empty area around the cells (e.g. a table with no rows).
    Blank,
    Rows,
    Column(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    RowStart,
    RowEnd,
    Top,
    Bottom,
}

/// Per-tab grid state.
#[derive(Debug, Clone)]
pub struct GridView {
    metrics: Metrics,
    /// Show the data after this many steps instead of after all of them.
    at: Option<usize>,
    columns: Vec<Column>,
    num_rows: usize,
    first_row: usize,
    /// Full rows that fit in the body.
    visible_rows: usize,
    body_width: f32,
    scroll_x: f32,
    selected: Option<(usize, usize)>,
    /// Selected item of the selected (open, list) cell.
    item: Option<usize>,
    /// Whole-row selection as (anchor, end), inclusive, set by clicking row
    /// numbers.
    rows: Option<(usize, usize)>,
    /// Whole-column selection as (anchor, end), inclusive.
    cols: Option<(usize, usize)>,
    /// First row of `cells`.
    window_start: usize,
    cells: Vec<Vec<Option<String>>>,
    /// JSON text of nested cells in `cells` (row-major, by column index),
    /// for editing; other cells are edited as displayed.
    json: Vec<Vec<Option<Option<String>>>>,
    /// Struct columns shown as their fields, by column key.
    expanded: BTreeSet<Vec<String>>,
    /// Read rows on another thread while scrolling (paged documents).
    background: bool,
    /// The read the app hasn't started yet.
    wanted: Option<RowRead>,
    /// The latest read started, and its rows; older results are dropped.
    reading: Option<(u64, Range<usize>)>,
    next_read: u64,
    /// Rows whose list and map cells show their items.
    open_rows: BTreeSet<usize>,
    /// Items of the open rows in `cells`: per screen column, (index or key,
    /// value) pairs, plus the number of items not listed.
    details: std::collections::HashMap<usize, Vec<Items>>,
    error: Option<String>,
    /// A temporary filter over the document's output (not a step).
    filter: Option<FilterView>,
}

/// A document column and the field names down to a field of it (empty
/// for the column itself).
pub type Target = (usize, Vec<String>);

/// The selected item of an open list or struct cell (see
/// [`GridView::selected_item`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedItem {
    /// Item index (the list's length for the new-item slot), or field index.
    pub index: usize,
    /// Number of items, or of fields.
    pub len: usize,
    /// The item type, or the field's type.
    pub data_type: DataType,
    /// The field name, for a struct field.
    pub field: Option<String>,
}

/// Cells whose items edit in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    List,
    Struct,
}

impl GridView {
    pub fn new(document: &Document, text_size: f32) -> Self {
        let schema = document
            .schema_at(document.evaluated_steps())
            .unwrap_or_else(|| document.schema());
        let columns = display_columns(&schema, &BTreeSet::new());
        let mut view = Self {
            at: None,
            metrics: Metrics::new(text_size),
            columns,
            num_rows: document.num_rows(),
            first_row: 0,
            visible_rows: 40,
            body_width: 800.0,
            scroll_x: 0.0,
            selected: None,
            item: None,
            rows: None,
            cols: None,
            window_start: 0,
            cells: Vec::new(),
            json: Vec::new(),
            expanded: BTreeSet::new(),
            background: false,
            wanted: None,
            reading: None,
            next_read: 0,
            open_rows: BTreeSet::new(),
            details: Default::default(),
            error: None,
            filter: None,
        };
        view.load(document, true);
        view.fit_columns();
        view
    }

    /// The selected cell as (row, document column).
    pub fn selected(&self) -> Option<(usize, usize)> {
        // A file with rows but no columns can have a row but no cell.
        let (r, c) = self.selected?;
        Some((r, self.columns.get(c)?.source))
    }

    /// Field names from the selected cell's document column down to it
    /// (empty unless it is a field of an expanded struct), and its type.
    pub fn selected_field(&self) -> Option<(&[String], &DataType)> {
        let column = self.columns.get(self.selected?.1)?;
        Some((column.path(), &column.data_type))
    }

    /// Text to edit the selected cell with (see [`Self::edit_value`]).
    pub fn selected_edit_value(&self) -> Option<Option<&str>> {
        let (row, column) = self.selected?;
        if let Some(item) = self.item {
            let items = self.items(row, column)?;
            return Some(match items.items.get(item) {
                Some(i) => i.edit.as_deref(),
                None => Some(""),
            });
        }
        self.edit_value(row, column)
    }

    /// The selected screen columns (or the selected cell's) as document
    /// columns with the field names down to them, and whether any is a
    /// field of an expanded struct.
    pub fn selected_targets(&self) -> Option<(Vec<Target>, bool)> {
        let range = self.selected_display_columns()?;
        Some(self.targets(range))
    }

    /// Targets from the first selected screen column to the last one shown,
    /// for pasting.
    pub fn paste_targets(&self) -> Option<(Vec<Target>, bool)> {
        let start = self.selected_display_columns()?.start;
        Some(self.targets(start..self.columns.len()))
    }

    fn targets(&self, range: Range<usize>) -> (Vec<Target>, bool) {
        let columns = self.columns.get(range).unwrap_or_default();
        let targets: Vec<_> = columns
            .iter()
            .map(|c| (c.source, c.path().to_vec()))
            .collect();
        let fields = targets.iter().any(|(_, path)| !path.is_empty());
        (targets, fields)
    }

    /// Screen columns showing document columns `sources`.
    fn display_range(&self, sources: Range<usize>) -> Range<usize> {
        let start = self
            .columns
            .iter()
            .position(|c| c.source >= sources.start)
            .unwrap_or(self.columns.len());
        let end = self
            .columns
            .iter()
            .position(|c| c.source >= sources.end)
            .unwrap_or(self.columns.len());
        start..end
    }

    /// Document columns shown by screen columns `range` (inclusive pair).
    fn source_range(&self, (a, b): (usize, usize)) -> Range<usize> {
        let (a, b) = (a.min(b), a.max(b));
        match (self.columns.get(a), self.columns.get(b)) {
            (Some(a), Some(b)) => a.source..b.source + 1,
            _ => 0..0,
        }
    }

    /// Selected screen columns, inclusive span.
    fn selected_display_columns(&self) -> Option<Range<usize>> {
        match (self.cols, self.selected) {
            (Some((a, b)), _) => Some(a.min(b)..a.max(b) + 1),
            (None, Some((_, c))) => Some(c..c + 1),
            (None, None) => None,
        }
    }

    /// Selected rows: the row selection, or the selected cell's row.
    pub fn selected_rows(&self) -> Option<Range<usize>> {
        match (self.rows, self.selected) {
            (Some((a, b)), _) => Some(a.min(b)..a.max(b) + 1),
            (None, Some((r, _))) => Some(r..r + 1),
            (None, None) => None,
        }
    }

    /// What is selected: whole rows, whole columns, or one cell.
    pub fn selection(&self) -> Selection {
        let span = |(a, b): (usize, usize)| a.min(b)..a.max(b) + 1;
        match (self.rows, self.cols, self.selected) {
            (Some(rows), _, _) => Selection::Rows(span(rows)),
            (None, Some(cols), _) => Selection::Columns(self.source_range(cols)),
            (None, None, Some((r, c))) => match self.columns.get(c) {
                Some(column) => Selection::Cell(r, column.source),
                None => Selection::Rows(r..r + 1),
            },
            (None, None, None) => Selection::None,
        }
    }

    /// Selected document columns: the column selection, or the selected
    /// cell's column.
    pub fn selected_columns(&self) -> Option<Range<usize>> {
        let range = self.selected_display_columns()?;
        Some(self.source_range((range.start, range.end - 1)))
    }

    /// Selects whole document columns `range` (e.g. after moving or adding
    /// one).
    pub fn select_columns(&mut self, range: Range<usize>) {
        let range = self.display_range(range);
        if range.is_empty() || range.end > self.columns.len() {
            return;
        }
        self.item = None;
        self.rows = None;
        self.cols = Some((range.start, range.end - 1));
        let row = self.selected.map_or(self.first_row, |(r, _)| r);
        if self.num_rows > 0 {
            self.selected = Some((row.min(self.num_rows - 1), range.start));
        }
        self.reveal(row.min(self.num_rows.saturating_sub(1)), range.start);
    }

    /// Selects whole rows `range` (e.g. after inserting them).
    pub fn select_rows(&mut self, range: Range<usize>, document: &Document) {
        if range.is_empty() || range.end > self.num_rows {
            return;
        }
        let column = self.selected.map_or(0, |(_, c)| c);
        self.item = None;
        self.rows = Some((range.start, range.end - 1));
        self.cols = None;
        self.selected = Some((range.start, column));
        self.reveal(
            range.start,
            column.min(self.columns.len().saturating_sub(1)),
        );
        self.load(document, false);
    }

    /// Text of a loaded cell (screen column): `Some(None)` for null, `None`
    /// if not loaded.
    pub fn value(&self, row: usize, column: usize) -> Option<Option<&str>> {
        self.cell(row, column).map(Option::as_deref)
    }

    /// Text to edit a loaded cell with: JSON for nested values, otherwise
    /// as displayed. `Some(None)` for null, `None` if not loaded.
    pub fn edit_value(&self, row: usize, column: usize) -> Option<Option<&str>> {
        let json = self
            .json
            .get(row.checked_sub(self.window_start)?)
            .and_then(|r| r.get(column))
            .and_then(Option::as_ref);
        match json {
            Some(value) => Some(value.as_deref()),
            None => self.value(row, column),
        }
    }

    /// The value of one cell (document column), as a one-element array.
    pub fn cell_array(
        &self,
        row: usize,
        column: usize,
        document: &Document,
    ) -> Option<veta_core::arrow::array::ArrayRef> {
        let batch = match self.view() {
            Some(view) => view.read(row..row + 1).ok()?,
            None => document.read_at(self.level(document), row..row + 1).ok()?,
        };
        (batch.num_rows() == 1 && column < batch.num_columns())
            .then(|| batch.column(column).clone())
    }

    /// Error from the last read, if it failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn apply(&mut self, event: GridEvent, document: &Document) {
        match event {
            GridEvent::Resized { rows, width } => {
                self.visible_rows = rows.max(1);
                self.body_width = width;
                self.set_scroll(self.first_row, self.scroll_x);
            }
            GridEvent::Scroll {
                first_row,
                scroll_x,
            } => self.set_scroll(first_row, scroll_x),
            GridEvent::SelectItem { row, column, item } => {
                if row < self.num_rows && column < self.columns.len() {
                    self.selected = Some((row, column));
                    self.select_item(Some(item), document);
                }
            }
            GridEvent::Select { row, column } => {
                if row < self.num_rows && column < self.columns.len() {
                    self.item = None;
                    self.selected = Some((row, column));
                    self.rows = None;
                    self.cols = None;
                    self.reveal(row, column);
                }
            }
            GridEvent::SelectRows { row, extend } => {
                self.item = None;
                if row < self.num_rows && !self.columns.is_empty() {
                    let anchor = match (extend, self.rows, self.selected) {
                        (true, Some((anchor, _)), _) => anchor,
                        (true, None, Some((r, _))) => r,
                        _ => row,
                    };
                    let column = self.selected.map_or(0, |(_, c)| c);
                    self.cols = None;
                    self.rows = Some((anchor, row));
                    self.selected = Some((row, column));
                    self.reveal(row, column);
                }
            }
            GridEvent::SelectColumns { column, extend } => {
                self.item = None;
                if column < self.columns.len() {
                    let anchor = match (extend, self.cols, self.selected) {
                        (true, Some((anchor, _)), _) => anchor,
                        (true, None, Some((_, c))) => c,
                        _ => column,
                    };
                    let row = self.selected.map_or(self.first_row, |(r, _)| r);
                    let row = row.min(self.num_rows.saturating_sub(1));
                    self.rows = None;
                    self.cols = Some((anchor, column));
                    if self.num_rows > 0 {
                        self.selected = Some((row, column));
                    }
                    self.reveal(row, column);
                }
            }
            GridEvent::Navigate(nav) => {
                self.rows = None;
                self.cols = None;
                if !self.navigate_items(nav) {
                    self.item = None;
                    self.navigate(nav);
                }
            }
            GridEvent::Expand { column } => {
                if let Some(c) = self.columns.get(column).filter(|c| c.expandable) {
                    let key = c.key.clone();
                    self.expanded.insert(key.clone());
                    self.refresh(document);
                    self.select_key(&key);
                }
            }
            GridEvent::Collapse { column } => {
                if let Some(key) = self.columns.get(column).and_then(|c| c.collapses.clone()) {
                    self.expanded.remove(&key);
                    self.refresh(document);
                    self.select_key(&key);
                }
            }
            GridEvent::ToggleItems { row } => {
                if !self.open_rows.remove(&row) && row < self.num_rows {
                    self.open_rows.insert(row);
                }
                self.load(document, true);
                self.reveal(row, self.selected.map_or(0, |(_, c)| c));
            }
            GridEvent::AutoFit { column } => {
                if column < self.columns.len() {
                    self.fit_column(column);
                    self.set_scroll(self.first_row, self.scroll_x);
                }
            }
            // Handled by the app, which turns them into commands.
            GridEvent::EditMove(_)
            | GridEvent::StartEdit { .. }
            | GridEvent::ClearCell
            | GridEvent::InsertRows
            | GridEvent::DeleteRows
            | GridEvent::MoveColumn { .. }
            | GridEvent::ContextMenu { .. } => {}
            GridEvent::ColumnResized { column, width } => {
                if let Some(c) = self.columns.get_mut(column) {
                    c.width = width.max(MIN_COLUMN_WIDTH);
                }
                self.set_scroll(self.first_row, self.scroll_x);
            }
        }
        self.load(document, false);
    }

    /// Selects the first screen column at or below `key` after expanding or
    /// collapsing it.
    fn select_key(&mut self, key: &[String]) {
        let Some(column) = self.columns.iter().position(|c| c.key.starts_with(key)) else {
            return;
        };
        self.cols = None;
        self.rows = None;
        if let Some((row, _)) = self.selected {
            self.selected = Some((row, column));
        }
        let row = self.selected.map_or(self.first_row, |(r, _)| r);
        self.reveal(row, column);
    }

    fn navigate(&mut self, nav: Nav) {
        if self.num_rows == 0 || self.columns.is_empty() {
            return;
        }
        let (row, column) = self.selected.unwrap_or((self.first_row, 0));
        let last_row = self.num_rows - 1;
        let last_column = self.columns.len() - 1;
        let page = self.visible_rows.max(1);
        let (row, column) = match nav {
            Nav::Up => (row.saturating_sub(1), column),
            Nav::Down => ((row + 1).min(last_row), column),
            Nav::Left => (row, column.saturating_sub(1)),
            Nav::Right => (row, (column + 1).min(last_column)),
            Nav::PageUp => (row.saturating_sub(page), column),
            Nav::PageDown => ((row + page).min(last_row), column),
            Nav::RowStart => (row, 0),
            Nav::RowEnd => (row, last_column),
            Nav::Top => (0, column),
            Nav::Bottom => (last_row, column),
        };
        self.selected = Some((row, column));
        self.reveal(row, column);
    }

    fn cell(&self, row: usize, column: usize) -> Option<&Option<String>> {
        self.cells
            .get(row.checked_sub(self.window_start)?)?
            .get(column)
    }
}
