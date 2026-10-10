//! Virtualized data grid.
//!
//! [`GridView`] is the per-tab view state (scroll position, selection, column
//! widths and the window of formatted rows currently on screen). [`Grid`] is
//! the widget that draws a `GridView` and turns mouse/keyboard input into
//! [`Event`]s, which the app feeds back into [`GridView::apply`]. Only the
//! rows in view (plus a margin) are ever read from the document.

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
use std::ops::Range;
use std::time::{Duration, Instant};
use veta_core::Document;

use crate::theme::Tokens;
use veta_core::display::{format_batch, is_numeric, type_name};

const SCROLLBAR: f32 = 12.0;
const MIN_THUMB: f32 = 24.0;
const CELL_PADDING: f32 = 6.0;
const MIN_COLUMN_WIDTH: f32 = 40.0;
const MIN_FITTED_WIDTH: f32 = 60.0;
const MAX_FITTED_WIDTH: f32 = 360.0;
const RESIZE_GRAB: f32 = 4.0;
const WHEEL_LINES: f32 = 3.0;
const NULL_TEXT: &str = "null";

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
    /// Right-click; `position` is in window coordinates.
    ContextMenu {
        target: MenuTarget,
        position: Point,
    },
}

/// What a context menu was opened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    Cell,
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

#[derive(Debug, Clone)]
struct Column {
    name: String,
    type_name: String,
    numeric: bool,
    width: f32,
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
    /// Whole-row selection as (anchor, end), inclusive, set by clicking row
    /// numbers.
    rows: Option<(usize, usize)>,
    /// Whole-column selection as (anchor, end), inclusive.
    cols: Option<(usize, usize)>,
    /// First row of `cells`.
    window_start: usize,
    cells: Vec<Vec<Option<String>>>,
    error: Option<String>,
}

impl GridView {
    pub fn new(document: &Document, text_size: f32) -> Self {
        let schema = document
            .schema_at(document.evaluated_steps())
            .unwrap_or_else(|| document.schema());
        let columns = schema
            .fields()
            .iter()
            .map(|f| Column {
                name: f.name().clone(),
                type_name: type_name(f.data_type()),
                numeric: is_numeric(f.data_type()),
                width: MIN_COLUMN_WIDTH,
            })
            .collect();
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
            rows: None,
            cols: None,
            window_start: 0,
            cells: Vec::new(),
            error: None,
        };
        view.load(document, true);
        view.fit_columns();
        view
    }

    /// Sizes each column to its header and the values currently loaded.
    fn fit_columns(&mut self) {
        for i in 0..self.columns.len() {
            self.fit_column(i);
        }
    }

    fn fit_column(&mut self, i: usize) {
        let column = &self.columns[i];
        let values = self
            .cells
            .iter()
            .filter_map(|row| row.get(i))
            .map(|v| v.as_deref().unwrap_or(NULL_TEXT).chars().count());
        let chars = values
            .chain([
                column.name.chars().count(),
                column.type_name.chars().count(),
            ])
            .max()
            .unwrap_or(0);
        self.columns[i].width = (chars as f32 * self.metrics.char_width + 2.0 * CELL_PADDING + 4.0)
            .clamp(MIN_FITTED_WIDTH, MAX_FITTED_WIDTH);
    }

    pub fn selected(&self) -> Option<(usize, usize)> {
        self.selected
    }

    /// Number of steps shown, if previewing an earlier step.
    pub fn preview(&self) -> Option<usize> {
        self.at
    }

    /// Shows the data after `at` steps (`None`: after all evaluated steps).
    pub fn set_preview(&mut self, at: Option<usize>, document: &Document) {
        self.at = at;
        self.refresh(document);
    }

    /// The step count whose output is shown.
    fn level(&self, document: &Document) -> usize {
        let evaluated = document.evaluated_steps();
        self.at.map_or(evaluated, |at| at.min(evaluated))
    }

    /// Selected rows: the row selection, or the selected cell's row.
    pub fn selected_rows(&self) -> Option<Range<usize>> {
        match (self.rows, self.selected) {
            (Some((a, b)), _) => Some(a.min(b)..a.max(b) + 1),
            (None, Some((r, _))) => Some(r..r + 1),
            (None, None) => None,
        }
    }

    /// Selected columns: the column selection, or the selected cell's column.
    pub fn selected_columns(&self) -> Option<Range<usize>> {
        match (self.cols, self.selected) {
            (Some((a, b)), _) => Some(a.min(b)..a.max(b) + 1),
            (None, Some((_, c))) => Some(c..c + 1),
            (None, None) => None,
        }
    }

    /// Selects whole columns `range` (e.g. after moving or adding one).
    pub fn select_columns(&mut self, range: Range<usize>) {
        if range.is_empty() || range.end > self.columns.len() {
            return;
        }
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
        self.rows = Some((range.start, range.end - 1));
        self.cols = None;
        self.selected = Some((range.start, column));
        self.reveal(
            range.start,
            column.min(self.columns.len().saturating_sub(1)),
        );
        self.load(document, false);
    }

    /// Text of a loaded cell: `Some(None)` for null, `None` if not loaded.
    pub fn value(&self, row: usize, column: usize) -> Option<Option<&str>> {
        self.cell(row, column).map(Option::as_deref)
    }

    /// Re-reads the document after it changed: columns (keeping the widths
    /// of columns that still exist), row count, selection and visible rows.
    pub fn refresh(&mut self, document: &Document) {
        let old: Vec<Column> = std::mem::take(&mut self.columns);
        let level = self.level(document);
        let schema = document
            .schema_at(level)
            .unwrap_or_else(|| document.schema());
        let mut fresh = Vec::new();
        self.columns = schema
            .fields()
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let kept = old.iter().find(|c| &c.name == f.name());
                if kept.is_none() {
                    fresh.push(i);
                }
                Column {
                    name: f.name().clone(),
                    type_name: type_name(f.data_type()),
                    numeric: is_numeric(f.data_type()),
                    width: kept.map_or(MIN_FITTED_WIDTH, |c| c.width),
                }
            })
            .collect();
        self.num_rows = document.num_rows_at(level).unwrap_or(0);
        self.selected = self.selected.and_then(|(r, c)| {
            (self.num_rows > 0 && !self.columns.is_empty())
                .then(|| (r.min(self.num_rows - 1), c.min(self.columns.len() - 1)))
        });
        let n = self.columns.len();
        self.cols = self
            .cols
            .and_then(|(a, b)| (n > 0).then(|| (a.min(n - 1), b.min(n - 1))));
        self.rows = self.rows.and_then(|(a, b)| {
            (self.num_rows > 0).then(|| (a.min(self.num_rows - 1), b.min(self.num_rows - 1)))
        });
        self.set_scroll(self.first_row, self.scroll_x);
        self.load(document, true);
        for i in fresh {
            self.fit_column(i);
        }
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
            GridEvent::Select { row, column } => {
                if row < self.num_rows && column < self.columns.len() {
                    self.selected = Some((row, column));
                    self.rows = None;
                    self.cols = None;
                    self.reveal(row, column);
                }
            }
            GridEvent::SelectRows { row, extend } => {
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
                self.navigate(nav);
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

    fn max_first_row(&self) -> usize {
        self.num_rows.saturating_sub(self.visible_rows)
    }

    fn content_width(&self) -> f32 {
        self.columns.iter().map(|c| c.width).sum()
    }

    fn max_scroll_x(&self) -> f32 {
        (self.content_width() - self.body_width).max(0.0)
    }

    fn set_scroll(&mut self, first_row: usize, scroll_x: f32) {
        self.first_row = first_row.min(self.max_first_row());
        self.scroll_x = scroll_x.clamp(0.0, self.max_scroll_x());
    }

    /// Scrolls the minimum needed for a cell to be fully visible.
    fn reveal(&mut self, row: usize, column: usize) {
        let mut first_row = self.first_row;
        if row < first_row {
            first_row = row;
        } else if row >= first_row + self.visible_rows {
            first_row = row + 1 - self.visible_rows;
        }
        let left: f32 = self.columns[..column].iter().map(|c| c.width).sum();
        let right = left + self.columns[column].width;
        let mut scroll_x = self.scroll_x;
        if left < scroll_x {
            scroll_x = left;
        } else if right > scroll_x + self.body_width {
            scroll_x = (right - self.body_width).min(left);
        }
        self.set_scroll(first_row, scroll_x);
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

    /// Makes sure `cells` covers the rows in view, reading a margin of one
    /// screen above and below so small scrolls don't hit the document.
    fn load(&mut self, document: &Document, force: bool) {
        let needed = self.first_row..(self.first_row + self.visible_rows + 1).min(self.num_rows);
        let window_end = self.window_start + self.cells.len();
        if !force && needed.start >= self.window_start && needed.end <= window_end {
            return;
        }
        let start = self.first_row.saturating_sub(self.visible_rows);
        let end = (self.first_row + 2 * self.visible_rows + 1).min(self.num_rows);
        match document
            .read_at(self.level(document), start..end)
            .and_then(|batch| format_batch(&batch))
        {
            Ok(cells) => {
                self.window_start = start;
                self.cells = cells;
                self.error = None;
            }
            Err(e) => {
                self.cells.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    fn cell(&self, row: usize, column: usize) -> Option<&Option<String>> {
        self.cells
            .get(row.checked_sub(self.window_start)?)?
            .get(column)
    }

    fn row_number_width(&self) -> f32 {
        let digits = self.num_rows.max(1).to_string().len() as f32;
        digits * self.metrics.char_width + 3.0 * CELL_PADDING
    }
}

/// The grid widget. Create with [`grid`].
pub struct Grid<'a, Message> {
    view: &'a GridView,
    tokens: Tokens,
    on_event: Box<dyn Fn(GridEvent) -> Message + 'a>,
    /// Editor drawn over the selected cell while it is being edited.
    editor: Option<Element<'a, Message>>,
}

impl<'a, Message> Grid<'a, Message> {
    /// Shows `editor` over the selected cell.
    pub fn editor(mut self, editor: Option<Element<'a, Message>>) -> Self {
        self.editor = editor;
        self
    }
}

impl<Message> std::fmt::Debug for Grid<'_, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grid")
            .field("view", self.view)
            .finish_non_exhaustive()
    }
}

pub fn grid<'a, Message>(
    view: &'a GridView,
    tokens: Tokens,
    on_event: impl Fn(GridEvent) -> Message + 'a,
) -> Grid<'a, Message> {
    Grid {
        view,
        tokens,
        on_event: Box::new(on_event),
        editor: None,
    }
}

#[derive(Debug, Default)]
struct State {
    focused: bool,
    drag: Drag,
    reported: Option<(usize, u32)>,
    /// Time and cell of the last click, to detect double-clicks.
    last_click: Option<(Instant, (usize, usize))>,
    /// Time and column of the last press on a column edge.
    last_edge_click: Option<(Instant, usize)>,
    modifiers: keyboard::Modifiers,
}

impl widget::operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

/// Id of the grid widget, for moving keyboard focus to it.
pub const GRID_ID: widget::Id = widget::Id::new("grid");
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Pixels a header must be dragged before it starts moving.
const DRAG_THRESHOLD: f32 = 6.0;

#[derive(Debug, Default, Clone, Copy)]
enum Drag {
    #[default]
    None,
    Column {
        column: usize,
        origin_x: f32,
        origin_width: f32,
    },
    /// Pressed on a column header; becomes a move once dragged far enough.
    Header {
        column: usize,
        origin_x: f32,
        x: f32,
        moving: bool,
    },
    VerticalThumb {
        grab: f32,
    },
    HorizontalThumb {
        grab: f32,
    },
}

/// Screen regions of the grid, computed from its bounds.
struct Regions {
    m: Metrics,
    bounds: Rectangle,
    header: Rectangle,
    row_numbers: Rectangle,
    body: Rectangle,
    v_track: Rectangle,
    h_track: Rectangle,
}

impl Regions {
    fn new(bounds: Rectangle, view: &GridView) -> Self {
        let m = view.metrics;
        let rn = view.row_number_width();
        let body = Rectangle {
            x: bounds.x + rn,
            y: bounds.y + m.header_height,
            width: (bounds.width - rn - SCROLLBAR).max(0.0),
            height: (bounds.height - m.header_height - SCROLLBAR).max(0.0),
        };
        Self {
            m,
            bounds,
            header: Rectangle {
                x: body.x,
                y: bounds.y,
                width: body.width,
                height: m.header_height,
            },
            row_numbers: Rectangle {
                x: bounds.x,
                y: body.y,
                width: rn,
                height: body.height,
            },
            v_track: Rectangle {
                x: body.x + body.width,
                y: body.y,
                width: SCROLLBAR,
                height: body.height,
            },
            h_track: Rectangle {
                x: body.x,
                y: body.y + body.height,
                width: body.width,
                height: SCROLLBAR,
            },
            body,
        }
    }

    fn full_rows(&self) -> usize {
        (self.body.height / self.m.row_height).floor() as usize
    }

    /// Vertical thumb (offset from track top, length).
    fn v_thumb(&self, view: &GridView) -> Option<(f32, f32)> {
        thumb(
            self.v_track.height,
            view.num_rows as f32,
            view.visible_rows as f32,
            view.first_row as f32,
        )
    }

    fn h_thumb(&self, view: &GridView) -> Option<(f32, f32)> {
        thumb(
            self.h_track.width,
            view.content_width(),
            self.body.width,
            view.scroll_x,
        )
    }
}

/// Thumb position and length for a scrollbar, or `None` if everything fits.
fn thumb(track: f32, total: f32, visible: f32, offset: f32) -> Option<(f32, f32)> {
    if total <= visible || track <= 0.0 {
        return None;
    }
    let length = (track * visible / total).clamp(MIN_THUMB.min(track), track);
    let max_offset = total - visible;
    let position = (track - length) * (offset / max_offset).clamp(0.0, 1.0);
    Some((position, length))
}

/// Inverse of [`thumb`]: offset for a thumb at `position` in the track.
fn offset_for_thumb(track: f32, length: f32, total: f32, visible: f32, position: f32) -> f32 {
    let free = track - length;
    if free <= 0.0 {
        return 0.0;
    }
    (position / free).clamp(0.0, 1.0) * (total - visible).max(0.0)
}

impl<Message> Grid<'_, Message> {
    /// Column index and its x range on screen, for columns intersecting the body.
    fn visible_columns(&self, body: Rectangle) -> Vec<(usize, f32, f32)> {
        let mut x = body.x - self.view.scroll_x;
        let mut out = Vec::new();
        for (i, c) in self.view.columns.iter().enumerate() {
            let right = x + c.width;
            if right > body.x && x < body.x + body.width {
                out.push((i, x, c.width));
            }
            if x > body.x + body.width {
                break;
            }
            x = right;
        }
        out
    }

    fn column_border_at(&self, regions: &Regions, position: Point) -> Option<usize> {
        if !regions.header.contains(position) {
            return None;
        }
        self.visible_columns(regions.body)
            .into_iter()
            .find(|&(_, x, w)| (position.x - (x + w)).abs() <= RESIZE_GRAB)
            .map(|(i, _, _)| i)
    }

    /// Screen rectangle of the selected cell, if it is visible.
    fn selected_cell_rect(&self, regions: &Regions) -> Option<Rectangle> {
        let (row, column) = self.view.selected?;
        let first = self.view.first_row;
        if row < first {
            return None;
        }
        let (_, x, width) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(c, _, _)| c == column)?;
        let rect = Rectangle {
            x,
            y: regions.body.y + (row - first) as f32 * regions.m.row_height,
            width,
            height: regions.m.row_height,
        };
        rect.intersection(&regions.body)
    }

    /// Row under a y coordinate in the body.
    fn row_at(&self, regions: &Regions, y: f32) -> Option<usize> {
        if y < regions.body.y || y >= regions.body.y + regions.body.height {
            return None;
        }
        let row = self.view.first_row + ((y - regions.body.y) / regions.m.row_height) as usize;
        (row < self.view.num_rows).then_some(row)
    }

    /// Gap (0 = before the first column, n = after the last) nearest to `x`,
    /// for dropping a dragged column.
    fn drop_slot(&self, regions: &Regions, x: f32) -> usize {
        let mut left = regions.body.x - self.view.scroll_x;
        for (i, c) in self.view.columns.iter().enumerate() {
            if x < left + c.width / 2.0 {
                return i;
            }
            left += c.width;
        }
        self.view.columns.len()
    }

    /// x of the gap `slot`, on screen.
    fn slot_x(&self, regions: &Regions, slot: usize) -> f32 {
        regions.body.x - self.view.scroll_x
            + self.view.columns[..slot]
                .iter()
                .map(|c| c.width)
                .sum::<f32>()
    }

    /// Column under an x coordinate.
    fn column_at(&self, regions: &Regions, x: f32) -> Option<usize> {
        self.visible_columns(regions.body)
            .into_iter()
            .find(|&(_, left, w)| x >= left && x < left + w)
            .map(|(i, _, _)| i)
    }

    fn cell_at(&self, regions: &Regions, position: Point) -> Option<(usize, usize)> {
        if !regions.body.contains(position) {
            return None;
        }
        let row =
            self.view.first_row + ((position.y - regions.body.y) / regions.m.row_height) as usize;
        let column = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(_, x, w)| position.x >= x && position.x < x + w)
            .map(|(i, _, _)| i)?;
        (row < self.view.num_rows).then_some((row, column))
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Grid<'_, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.max();
        if self.editor.is_none() {
            return layout::Node::new(size);
        }
        // Lay the editor out over the selected cell (local coordinates); off
        // screen when the cell is scrolled out of view.
        let regions = Regions::new(Rectangle::new(Point::ORIGIN, size), self.view);
        let rect = self.selected_cell_rect(&regions).unwrap_or(Rectangle::new(
            Point::new(-10_000.0, -10_000.0),
            Size::new(1.0, 1.0),
        ));
        let Some(editor) = &mut self.editor else {
            return layout::Node::new(size);
        };
        let child_limits = layout::Limits::new(Size::ZERO, rect.size());
        let child = editor
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &child_limits)
            .move_to(rect.position());
        layout::Node::with_children(size, vec![child])
    }

    fn children(&self) -> Vec<Tree> {
        self.editor.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        match &self.editor {
            Some(editor) => tree.diff_children(std::slice::from_ref(editor)),
            None => tree.children.clear(),
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(Some(&GRID_ID), layout.bounds(), state);
        if let (Some(editor), Some(child_layout)) = (&mut self.editor, layout.children().next()) {
            operation.traverse(&mut |operation| {
                editor.as_widget_mut().operate(
                    &mut tree.children[0],
                    child_layout,
                    renderer,
                    operation,
                );
            });
        }
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let (Some(editor), Some(child_layout)) = (&mut self.editor, layout.children().next()) {
            editor.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
            if shell.is_event_captured() {
                return;
            }
            // Keys the text input doesn't use apply the edit and move.
            if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
                let nav = match key.as_ref() {
                    keyboard::Key::Named(Named::ArrowUp) => Some(Nav::Up),
                    keyboard::Key::Named(Named::ArrowDown) => Some(Nav::Down),
                    keyboard::Key::Named(Named::Tab) if modifiers.shift() => Some(Nav::Left),
                    keyboard::Key::Named(Named::Tab) => Some(Nav::Right),
                    _ => None,
                };
                if let Some(nav) = nav {
                    shell.publish((self.on_event)(GridEvent::EditMove(nav)));
                    shell.capture_event();
                    return;
                }
            }
        }
        let state = tree.state.downcast_mut::<State>();
        let regions = Regions::new(layout.bounds(), self.view);
        let view = self.view;

        // Report the body size whenever it changes so the app reads the
        // right number of rows.
        let size = (regions.full_rows(), regions.body.width.round() as u32);
        if state.reported != Some(size) {
            state.reported = Some(size);
            shell.publish((self.on_event)(GridEvent::Resized {
                rows: size.0,
                width: regions.body.width,
            }));
        }

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if !cursor.is_over(regions.bounds) {
                    return;
                }
                let (dx, dy) = match *delta {
                    ScrollDelta::Lines { x, y } => {
                        (x * 40.0, y * WHEEL_LINES * regions.m.row_height)
                    }
                    ScrollDelta::Pixels { x, y } => (x, y),
                };
                let rows = (-dy / regions.m.row_height).round() as isize;
                let first_row = view.first_row.saturating_add_signed(rows);
                shell.publish((self.on_event)(GridEvent::Scroll {
                    first_row,
                    scroll_x: view.scroll_x - dx,
                }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_over(regions.bounds) else {
                    state.focused = false;
                    return;
                };
                state.focused = true;
                shell.capture_event();

                if regions.row_numbers.contains(position) {
                    if let Some(row) = self.row_at(&regions, position.y) {
                        shell.publish((self.on_event)(GridEvent::SelectRows {
                            row,
                            extend: state.modifiers.shift(),
                        }));
                    }
                } else if let Some(column) = self.column_border_at(&regions, position) {
                    let now = Instant::now();
                    let double = state.last_edge_click.is_some_and(|(at, c)| {
                        c == column && now.duration_since(at) < DOUBLE_CLICK
                    });
                    if double {
                        state.last_edge_click = None;
                        shell.publish((self.on_event)(GridEvent::AutoFit { column }));
                    } else {
                        state.last_edge_click = Some((now, column));
                        state.drag = Drag::Column {
                            column,
                            origin_x: position.x,
                            origin_width: view.columns[column].width,
                        };
                    }
                } else if regions.header.contains(position) {
                    if let Some(column) = self.column_at(&regions, position.x) {
                        shell.publish((self.on_event)(GridEvent::SelectColumns {
                            column,
                            extend: state.modifiers.shift(),
                        }));
                        state.drag = Drag::Header {
                            column,
                            origin_x: position.x,
                            x: position.x,
                            moving: false,
                        };
                    }
                } else if regions.v_track.contains(position) {
                    if let Some((offset, length)) = regions.v_thumb(view) {
                        let local = position.y - regions.v_track.y;
                        if local >= offset && local <= offset + length {
                            state.drag = Drag::VerticalThumb {
                                grab: local - offset,
                            };
                        } else {
                            let page = view.visible_rows.max(1);
                            let first_row = if local < offset {
                                view.first_row.saturating_sub(page)
                            } else {
                                view.first_row + page
                            };
                            shell.publish((self.on_event)(GridEvent::Scroll {
                                first_row,
                                scroll_x: view.scroll_x,
                            }));
                        }
                    }
                } else if regions.h_track.contains(position) {
                    if let Some((offset, length)) = regions.h_thumb(view) {
                        let local = position.x - regions.h_track.x;
                        if local >= offset && local <= offset + length {
                            state.drag = Drag::HorizontalThumb {
                                grab: local - offset,
                            };
                        } else {
                            let page = regions.body.width;
                            let scroll_x = if local < offset {
                                view.scroll_x - page
                            } else {
                                view.scroll_x + page
                            };
                            shell.publish((self.on_event)(GridEvent::Scroll {
                                first_row: view.first_row,
                                scroll_x,
                            }));
                        }
                    }
                } else if let Some((row, column)) = self.cell_at(&regions, position) {
                    shell.publish((self.on_event)(GridEvent::Select { row, column }));
                    let now = Instant::now();
                    let double = state.last_click.is_some_and(|(at, cell)| {
                        cell == (row, column) && now.duration_since(at) < DOUBLE_CLICK
                    });
                    if double {
                        state.last_click = None;
                        shell.publish((self.on_event)(GridEvent::StartEdit { initial: None }));
                    } else {
                        state.last_click = Some((now, (row, column)));
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let Some(position) = cursor.position_over(regions.bounds) else {
                    return;
                };
                state.focused = true;
                shell.capture_event();
                let in_rows = |row: usize| view.selected_rows().is_some_and(|r| r.contains(&row));
                let target = if let Some((row, column)) = self.cell_at(&regions, position) {
                    if !(view.rows.is_some() && in_rows(row)) {
                        shell.publish((self.on_event)(GridEvent::Select { row, column }));
                    }
                    Some(MenuTarget::Cell)
                } else if regions.row_numbers.contains(position) {
                    self.row_at(&regions, position.y).map(|row| {
                        if !(view.rows.is_some() && in_rows(row)) {
                            shell.publish((self.on_event)(GridEvent::SelectRows {
                                row,
                                extend: false,
                            }));
                        }
                        MenuTarget::Rows
                    })
                } else if regions.header.contains(position) {
                    self.column_at(&regions, position.x).map(MenuTarget::Column)
                } else {
                    None
                };
                if let Some(target) = target {
                    shell.publish((self.on_event)(GridEvent::ContextMenu { target, position }));
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => match state.drag {
                Drag::None => {}
                Drag::Header {
                    column,
                    origin_x,
                    moving,
                    ..
                } => {
                    state.drag = Drag::Header {
                        column,
                        origin_x,
                        x: position.x,
                        moving: moving || (position.x - origin_x).abs() > DRAG_THRESHOLD,
                    };
                    shell.request_redraw();
                }
                Drag::Column {
                    column,
                    origin_x,
                    origin_width,
                } => {
                    shell.publish((self.on_event)(GridEvent::ColumnResized {
                        column,
                        width: (origin_width + position.x - origin_x).max(MIN_COLUMN_WIDTH),
                    }));
                }
                Drag::VerticalThumb { grab } => {
                    if let Some((_, length)) = regions.v_thumb(view) {
                        let offset = offset_for_thumb(
                            regions.v_track.height,
                            length,
                            view.num_rows as f32,
                            view.visible_rows as f32,
                            position.y - regions.v_track.y - grab,
                        );
                        shell.publish((self.on_event)(GridEvent::Scroll {
                            first_row: offset.round() as usize,
                            scroll_x: view.scroll_x,
                        }));
                    }
                }
                Drag::HorizontalThumb { grab } => {
                    if let Some((_, length)) = regions.h_thumb(view) {
                        let scroll_x = offset_for_thumb(
                            regions.h_track.width,
                            length,
                            view.content_width(),
                            regions.body.width,
                            position.x - regions.h_track.x - grab,
                        );
                        shell.publish((self.on_event)(GridEvent::Scroll {
                            first_row: view.first_row,
                            scroll_x,
                        }));
                    }
                }
            },
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Drag::Header {
                    column,
                    x,
                    moving: true,
                    ..
                } = state.drag
                {
                    let slot = self.drop_slot(&regions, x);
                    let to = if slot > column { slot - 1 } else { slot };
                    if to != column {
                        shell.publish((self.on_event)(GridEvent::MoveColumn { from: column, to }));
                    }
                }
                state.drag = Drag::None;
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            }) if state.focused => {
                let ctrl = modifiers.command();
                let rows = match key.as_ref() {
                    keyboard::Key::Character("-") if ctrl => Some(GridEvent::DeleteRows),
                    keyboard::Key::Character("+" | "=") if ctrl => Some(GridEvent::InsertRows),
                    _ => None,
                };
                if let Some(event) = rows {
                    if view.selected.is_some() {
                        shell.publish((self.on_event)(event));
                        shell.capture_event();
                    }
                    return;
                }
                let edit = match key.as_ref() {
                    keyboard::Key::Named(Named::Enter | Named::F2) => {
                        Some(GridEvent::StartEdit { initial: None })
                    }
                    keyboard::Key::Named(Named::Delete) => Some(GridEvent::ClearCell),
                    keyboard::Key::Named(Named::Backspace) => Some(GridEvent::StartEdit {
                        initial: Some(String::new()),
                    }),
                    _ if !ctrl && !modifiers.alt() => text
                        .as_ref()
                        .filter(|t| t.chars().all(|c| !c.is_control()) && !t.is_empty())
                        .map(|t| GridEvent::StartEdit {
                            initial: Some(t.to_string()),
                        }),
                    _ => None,
                };
                if let Some(event) = edit {
                    if view.selected.is_some() {
                        shell.publish((self.on_event)(event));
                        shell.capture_event();
                    }
                    return;
                }
                let nav = match key.as_ref() {
                    keyboard::Key::Named(Named::ArrowUp) => Some(Nav::Up),
                    keyboard::Key::Named(Named::ArrowDown) => Some(Nav::Down),
                    keyboard::Key::Named(Named::ArrowLeft) => Some(Nav::Left),
                    keyboard::Key::Named(Named::ArrowRight) => Some(Nav::Right),
                    keyboard::Key::Named(Named::PageUp) => Some(Nav::PageUp),
                    keyboard::Key::Named(Named::PageDown) => Some(Nav::PageDown),
                    keyboard::Key::Named(Named::Home) if ctrl => Some(Nav::Top),
                    keyboard::Key::Named(Named::End) if ctrl => Some(Nav::Bottom),
                    keyboard::Key::Named(Named::Home) => Some(Nav::RowStart),
                    keyboard::Key::Named(Named::End) => Some(Nav::RowEnd),
                    _ => None,
                };
                if let Some(nav) = nav {
                    shell.publish((self.on_event)(GridEvent::Navigate(nav)));
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if let (Some(editor), Some(child_layout)) = (&self.editor, layout.children().next())
            && cursor.is_over(child_layout.bounds())
        {
            return editor.as_widget().mouse_interaction(
                &tree.children[0],
                child_layout,
                cursor,
                viewport,
                renderer,
            );
        }
        let state = tree.state.downcast_ref::<State>();
        match state.drag {
            Drag::Column { .. } => return mouse::Interaction::ResizingHorizontally,
            Drag::Header { moving: true, .. } => return mouse::Interaction::Grabbing,
            _ => {}
        }
        let regions = Regions::new(layout.bounds(), self.view);
        match cursor.position() {
            Some(p) if self.column_border_at(&regions, p).is_some() => {
                mouse::Interaction::ResizingHorizontally
            }
            Some(p) if regions.body.contains(p) => mouse::Interaction::Cell,
            _ => mouse::Interaction::None,
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let view = self.view;
        let regions = Regions::new(layout.bounds(), view);
        let m = regions.m;
        let t = &self.tokens;
        let text_color = t.text;
        let muted = t.muted_text;
        let line = t.border;
        let header_bg = t.grid_header;
        let stripe = t.grid_stripe;
        let bold = Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        };

        fill(renderer, regions.bounds, t.background);

        let columns = self.visible_columns(regions.body);
        // Whole-row selection only; a single selected cell is outlined below.
        let selected_rows = if view.rows.is_some() {
            view.selected_rows().unwrap_or(0..0)
        } else {
            0..0
        };
        let rows_on_screen = (regions.body.height / m.row_height).ceil() as usize;
        let last_row = (view.first_row + rows_on_screen).min(view.num_rows);

        // Body: stripes, selection, cells.
        for row in view.first_row..last_row {
            let y = regions.body.y + (row - view.first_row) as f32 * m.row_height;
            let row_rect = Rectangle {
                x: regions.body.x,
                y,
                width: regions.body.width,
                height: m.row_height,
            }
            .intersection(&regions.body);
            let Some(row_rect) = row_rect else { continue };
            if row % 2 == 1 {
                fill(renderer, row_rect, stripe);
            }
            if selected_rows.contains(&row) {
                fill(
                    renderer,
                    row_rect,
                    Color {
                        a: 0.14,
                        ..t.selection
                    },
                );
            }
            for &(column, x, width) in &columns {
                let cell = Rectangle {
                    x,
                    y,
                    width,
                    height: m.row_height,
                };
                let Some(clip) = cell.intersection(&regions.body) else {
                    continue;
                };
                let (content, color) = match view.cell(row, column) {
                    Some(Some(value)) => (single_line(value), text_color),
                    Some(None) => (NULL_TEXT.to_owned(), muted),
                    None => (String::new(), text_color),
                };
                let numeric = view.columns[column].numeric;
                draw_text(
                    renderer,
                    content,
                    cell,
                    numeric,
                    m.text_size,
                    Font::DEFAULT,
                    color,
                    clip,
                );
            }
        }

        // Vertical grid lines in the body and header.
        for &(_, x, width) in &columns {
            let line_x = x + width - 1.0;
            if line_x >= regions.body.x && line_x < regions.body.x + regions.body.width {
                fill(
                    renderer,
                    Rectangle {
                        x: line_x,
                        y: regions.bounds.y,
                        width: 1.0,
                        height: m.header_height + regions.body.height,
                    },
                    line,
                );
            }
        }

        // Selection outline.
        if let Some((row, column)) = view.selected
            && row >= view.first_row
            && row < last_row
            && let Some(&(_, x, width)) = columns.iter().find(|(c, _, _)| *c == column)
        {
            let rect = Rectangle {
                x,
                y: regions.body.y + (row - view.first_row) as f32 * m.row_height,
                width,
                height: m.row_height,
            };
            if let Some(clip) = rect.intersection(&regions.body) {
                renderer.fill_quad(
                    Quad {
                        bounds: clip,
                        border: Border {
                            color: t.selection,
                            width: 2.0,
                            radius: 0.0.into(),
                        },
                        ..Quad::default()
                    },
                    Color::TRANSPARENT,
                );
            }
        }

        // Header.
        fill(renderer, regions.header, header_bg);
        let selected_cols = if view.cols.is_some() {
            view.selected_columns().unwrap_or(0..0)
        } else {
            0..0
        };
        for &(column, x, width) in &columns {
            if selected_cols.contains(&column) {
                let highlight = Color {
                    a: 0.14,
                    ..t.selection
                };
                let header = Rectangle {
                    x,
                    y: regions.header.y,
                    width,
                    height: m.header_height,
                };
                let body = Rectangle {
                    x,
                    y: regions.body.y,
                    width,
                    height: regions.body.height,
                };
                for rect in [header, body] {
                    if let Some(clip) = rect.intersection(&regions.bounds) {
                        fill(renderer, clip, highlight);
                    }
                }
            }
        }
        for &(column, x, width) in &columns {
            let c = &view.columns[column];
            let cell = Rectangle {
                x,
                y: regions.bounds.y + 3.0,
                width,
                height: m.header_height / 2.0,
            };
            let Some(clip) = cell.intersection(&regions.header) else {
                continue;
            };
            draw_text(
                renderer,
                c.name.clone(),
                cell,
                false,
                m.text_size,
                bold,
                text_color,
                clip,
            );
            let type_cell = Rectangle {
                y: cell.y + m.header_height / 2.0 - 4.0,
                ..cell
            };
            if let Some(clip) = type_cell.intersection(&regions.header) {
                draw_text(
                    renderer,
                    c.type_name.clone(),
                    type_cell,
                    false,
                    m.type_text_size,
                    Font::DEFAULT,
                    muted,
                    clip,
                );
            }
        }
        fill(
            renderer,
            Rectangle {
                x: regions.bounds.x,
                y: regions.header.y + m.header_height - 1.0,
                width: regions.bounds.width,
                height: 1.0,
            },
            muted,
        );

        // Row numbers.
        fill(renderer, regions.row_numbers, header_bg);
        let corner = Rectangle {
            x: regions.bounds.x,
            y: regions.bounds.y,
            width: regions.row_numbers.width,
            height: m.header_height - 1.0,
        };
        fill(renderer, corner, header_bg);
        let highlighted = view.selected_rows().unwrap_or(0..0);
        for row in view.first_row..last_row {
            let cell = Rectangle {
                x: regions.row_numbers.x,
                y: regions.body.y + (row - view.first_row) as f32 * m.row_height,
                width: regions.row_numbers.width - CELL_PADDING / 2.0,
                height: m.row_height,
            };
            let Some(clip) = cell.intersection(&regions.row_numbers) else {
                continue;
            };
            let (font, color) = if highlighted.contains(&row) {
                (bold, t.selection)
            } else {
                (Font::DEFAULT, muted)
            };
            draw_text(
                renderer,
                (row + 1).to_string(),
                cell,
                true,
                m.text_size,
                font,
                color,
                clip,
            );
        }

        // Scrollbars.
        let track = t.grid_header;
        let thumb_color = t.border;
        fill(renderer, regions.v_track, track);
        fill(renderer, regions.h_track, track);
        fill(
            renderer,
            Rectangle {
                x: regions.v_track.x,
                y: regions.h_track.y,
                width: SCROLLBAR,
                height: SCROLLBAR,
            },
            track,
        );
        if let Some((offset, length)) = regions.v_thumb(view) {
            rounded(
                renderer,
                Rectangle {
                    x: regions.v_track.x + 2.0,
                    y: regions.v_track.y + offset,
                    width: SCROLLBAR - 4.0,
                    height: length,
                },
                thumb_color,
            );
        }
        if let Some((offset, length)) = regions.h_thumb(view) {
            rounded(
                renderer,
                Rectangle {
                    x: regions.h_track.x + offset,
                    y: regions.h_track.y + 2.0,
                    width: length,
                    height: SCROLLBAR - 4.0,
                },
                thumb_color,
            );
        }

        let state = tree.state.downcast_ref::<State>();
        if let Drag::Header {
            x, moving: true, ..
        } = state.drag
        {
            let line_x = self.slot_x(&regions, self.drop_slot(&regions, x));
            fill(
                renderer,
                Rectangle {
                    x: line_x - 1.0,
                    y: regions.bounds.y,
                    width: 3.0,
                    height: m.header_height + regions.body.height,
                },
                t.selection,
            );
        }

        if let (Some(editor), Some(child_layout)) = (&self.editor, layout.children().next()) {
            renderer.with_layer(regions.body, |renderer| {
                editor.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    child_layout,
                    cursor,
                    viewport,
                );
            });
        }

        if view.columns.is_empty() {
            draw_text(
                renderer,
                "This file has no columns.".into(),
                regions.body,
                false,
                m.text_size,
                Font::DEFAULT,
                muted,
                regions.body,
            );
        }
    }
}

impl<'a, Message: 'a> From<Grid<'a, Message>> for Element<'a, Message> {
    fn from(grid: Grid<'a, Message>) -> Self {
        Element::new(grid)
    }
}

fn fill(renderer: &mut Renderer, bounds: Rectangle, color: Color) {
    renderer.fill_quad(
        Quad {
            bounds,
            ..Quad::default()
        },
        color,
    );
}

fn rounded(renderer: &mut Renderer, bounds: Rectangle, color: Color) {
    renderer.fill_quad(
        Quad {
            bounds,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..Quad::default()
        },
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    renderer: &mut Renderer,
    content: String,
    cell: Rectangle,
    right: bool,
    size: f32,
    font: Font,
    color: Color,
    clip: Rectangle,
) {
    let x = if right {
        cell.x + cell.width - CELL_PADDING
    } else {
        cell.x + CELL_PADDING
    };
    renderer.fill_text(
        Text {
            content,
            bounds: Size::new(f32::INFINITY, cell.height),
            size: Pixels(size),
            line_height: text::LineHeight::default(),
            font,
            align_x: if right {
                text::Alignment::Right
            } else {
                text::Alignment::Left
            },
            align_y: Vertical::Center,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::None,
        },
        Point::new(x, cell.center_y()),
        color,
        clip,
    );
}

/// Cells show one line; newlines and tabs become spaces.
fn single_line(value: &str) -> String {
    if value.contains(['\n', '\r', '\t']) {
        value.replace(['\n', '\r', '\t'], " ")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
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
}
