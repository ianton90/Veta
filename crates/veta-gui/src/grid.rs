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
use std::collections::BTreeSet;
use std::ops::Range;
use std::time::{Duration, Instant};
use veta_core::Document;
use veta_core::arrow::array::Array;
use veta_core::arrow::datatypes::{DataType, Field, Schema};
use veta_core::arrow::record_batch::{RecordBatch, RecordBatchOptions};
use veta_core::steps::{ComputeJob, Computed, Filter, Pipeline, Status};

use crate::icon::{self, Icon};
use crate::theme::Tokens;
use veta_core::display::{format_batch, has_items, is_numeric, type_name};

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

/// A column on screen: a document column, or a field of an expanded struct
/// column.
#[derive(Debug, Clone)]
struct Column {
    /// The document column's name followed by the field names down to this
    /// column.
    key: Vec<String>,
    /// Index of the document column.
    source: usize,
    name: String,
    data_type: DataType,
    type_name: String,
    numeric: bool,
    width: f32,
    /// A struct whose fields aren't shown.
    expandable: bool,
    /// Lists or maps: cells show a summary and can open into items.
    items: bool,
    /// Lists whose items can be edited, added and removed in place.
    list: bool,
    /// Structs: cells open into their fields, edited in place.
    is_struct: bool,
    /// The expanded struct whose first field this is (collapse button).
    collapses: Option<Vec<String>>,
}

impl Column {
    /// Field names below the document column.
    fn path(&self) -> &[String] {
        &self.key[1..]
    }
}

/// The columns on screen for `schema`, with the structs in `expanded` shown
/// as their fields.
fn display_columns(schema: &Schema, expanded: &BTreeSet<Vec<String>>) -> Vec<Column> {
    fn push(
        out: &mut Vec<Column>,
        source: usize,
        key: Vec<String>,
        field: &Field,
        collapses: Option<Vec<String>>,
        expanded: &BTreeSet<Vec<String>>,
    ) {
        let data_type = field.data_type();
        if let DataType::Struct(children) = data_type
            && !children.is_empty()
            && expanded.contains(&key)
        {
            for (i, child) in children.iter().enumerate() {
                let mut child_key = key.clone();
                child_key.push(child.name().clone());
                let starts = (i == 0).then(|| key.clone());
                push(out, source, child_key, child, starts, expanded);
            }
            return;
        }
        out.push(Column {
            name: key.join("."),
            source,
            expandable: matches!(data_type, DataType::Struct(c) if !c.is_empty()),
            items: has_items(data_type),
            list: matches!(data_type, DataType::List(_) | DataType::LargeList(_)),
            is_struct: matches!(data_type, DataType::Struct(c) if !c.is_empty()),
            data_type: data_type.clone(),
            type_name: type_name(data_type),
            numeric: is_numeric(data_type),
            width: MIN_COLUMN_WIDTH,
            collapses,
            key,
        });
    }
    let mut out = Vec::new();
    for (i, field) in schema.fields().iter().enumerate() {
        push(
            &mut out,
            i,
            vec![field.name().clone()],
            field,
            None,
            expanded,
        );
    }
    out
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

/// What kind of value an open cell lists the items of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ItemsKind {
    /// Not a list, map or struct: nothing listed.
    #[default]
    None,
    /// An editable list (see [`Column::list`]).
    List,
    Map,
    /// Struct fields, editable.
    Struct,
}

/// The items listed in an open list, map or struct cell.
#[derive(Debug, Clone, Default)]
struct Items {
    items: Vec<veta_core::display::ListItem>,
    more: usize,
    kind: ItemsKind,
}

impl Items {
    /// Screen lines the items take, with the slot for a new item when the
    /// list is empty or `appending`.
    fn lines(&self, appending: bool) -> usize {
        self.items.len() + usize::from(self.more > 0 || self.has_slot(appending))
    }

    /// Whether the slot for a new item is shown.
    fn has_slot(&self, appending: bool) -> bool {
        self.kind == ItemsKind::List && self.more == 0 && (self.items.is_empty() || appending)
    }

    /// Items that can be selected: the items and the slot, if shown.
    fn selectable(&self, appending: bool) -> usize {
        match self.kind {
            ItemsKind::List => self.items.len() + usize::from(self.has_slot(appending)),
            ItemsKind::Struct => self.items.len(),
            ItemsKind::None | ItemsKind::Map => 0,
        }
    }
}

/// The document's output with a filter applied, computed in the background.
#[derive(Debug, Clone)]
struct FilterView {
    filter: Filter,
    pipeline: Pipeline,
}

impl FilterView {
    fn ready(&self) -> bool {
        self.pipeline.status() == &Status::Ready
    }
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
            open_rows: BTreeSet::new(),
            details: Default::default(),
            error: None,
            filter: None,
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
        let toggles = usize::from(column.expandable) + usize::from(column.collapses.is_some());
        let header = column.name.chars().count() as f32 * self.metrics.char_width
            + toggles as f32 * (self.metrics.text_size + 2.0);
        let chars = values
            .chain([column.type_name.chars().count()])
            .max()
            .unwrap_or(0);
        let chevron = if column.items {
            self.metrics.text_size + 2.0
        } else {
            0.0
        };
        let content = (chars as f32 * self.metrics.char_width + chevron).max(header);
        self.columns[i].width =
            (content + 2.0 * CELL_PADDING + 4.0).clamp(MIN_FITTED_WIDTH, MAX_FITTED_WIDTH);
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

    /// The selected list item: its index, the number of items and the item
    /// type. The list is at [`Self::selected_field`] in the cell. The index
    /// equals the number of items for the slot of a new item.
    pub fn selected_item(&self) -> Option<SelectedItem> {
        let (row, column) = self.selected?;
        let index = self.item?;
        let len = self.items(row, column)?.items.len();
        match &self.columns.get(column)?.data_type {
            DataType::List(f) | DataType::LargeList(f) => Some(SelectedItem {
                index,
                len,
                data_type: f.data_type().clone(),
                field: None,
            }),
            DataType::Struct(fields) => {
                let field = fields.get(index)?;
                Some(SelectedItem {
                    index,
                    len,
                    data_type: field.data_type().clone(),
                    field: Some(field.name().clone()),
                })
            }
            _ => None,
        }
    }

    /// Whether the selected cell is a list or struct whose items (fields)
    /// can be edited in place.
    pub fn selected_opens_items(&self) -> Option<OpenKind> {
        let column = self.columns.get(self.selected?.1)?;
        if column.list {
            Some(OpenKind::List)
        } else if column.is_struct {
            Some(OpenKind::Struct)
        } else {
            None
        }
    }

    /// Opens the selected cell's row and selects item `item` of it (`None`:
    /// the cell itself). An item past the end selects the new-item slot.
    pub fn select_item(&mut self, item: Option<usize>, document: &Document) {
        let Some((row, column)) = self.selected else {
            return;
        };
        if item.is_some() && !self.open_rows.contains(&row) {
            self.open_rows.insert(row);
            self.load(document, true);
        }
        let len = self.items(row, column).map(|i| i.items.len());
        self.item = match (item, len) {
            (Some(i), Some(len)) => Some(i.min(len)),
            _ => None,
        };
        self.rows = None;
        self.cols = None;
        self.reveal(row, column);
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

    /// Number of steps shown, if previewing an earlier step.
    pub fn preview(&self) -> Option<usize> {
        self.at
    }

    /// Shows the data after `at` steps (`None`: after all evaluated steps).
    /// Clears the filter, if any.
    pub fn set_preview(&mut self, at: Option<usize>, document: &Document) {
        // Other rows show after another step or without the filter.
        if at != self.at || self.filter.is_some() {
            self.open_rows.clear();
        }
        self.at = at;
        self.filter = None;
        self.refresh(document);
    }

    /// The temporary filter, if one is on.
    pub fn filter(&self) -> Option<&Filter> {
        self.filter.as_ref().map(|f| &f.filter)
    }

    /// Shows the document filtered by `view` (a filter and the document's
    /// pipeline with it applied), or unfiltered.
    pub fn set_filter(&mut self, view: Option<(Filter, Pipeline)>, document: &Document) {
        self.at = None;
        self.filter = view.map(|(filter, pipeline)| FilterView { filter, pipeline });
        self.open_rows.clear();
        self.refresh(document);
    }

    /// The filter's pass, while it still needs computing.
    pub fn filter_job(&self) -> Option<ComputeJob> {
        self.filter.as_ref()?.pipeline.compute_job()
    }

    pub fn filter_key(&self) -> Option<Vec<u64>> {
        self.filter.as_ref()?.pipeline.compute_key()
    }

    /// Stores the filter's computed pass; returns whether it applied.
    pub fn install_filter(&mut self, computed: Computed, document: &Document) -> bool {
        let Some(view) = &mut self.filter else {
            return false;
        };
        if !view.pipeline.install(computed) {
            return false;
        }
        self.open_rows.clear();
        self.refresh(document);
        true
    }

    /// Rows shown and rows in the document, while a filter is on and
    /// computed.
    pub fn filter_counts(&self, document: &Document) -> Option<(usize, usize)> {
        let view = self.filter.as_ref().filter(|f| f.ready())?;
        Some((view.pipeline.num_rows(), document.num_rows()))
    }

    /// The computed filter view, if any.
    fn view(&self) -> Option<&Pipeline> {
        self.filter
            .as_ref()
            .filter(|f| f.ready())
            .map(|f| &f.pipeline)
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

    /// Re-reads the document after it changed: columns (keeping the widths
    /// of columns that still exist), row count, selection and visible rows.
    pub fn refresh(&mut self, document: &Document) {
        let old: Vec<Column> = std::mem::take(&mut self.columns);
        let level = self.level(document);
        let schema = match self.view() {
            Some(view) => view.schema(),
            None => document
                .schema_at(level)
                .unwrap_or_else(|| document.schema()),
        };
        let mut fresh = Vec::new();
        self.columns = display_columns(&schema, &self.expanded);
        for (i, column) in self.columns.iter_mut().enumerate() {
            // A column whose type changed is fitted again.
            match old
                .iter()
                .find(|c| c.key == column.key && c.type_name == column.type_name)
            {
                Some(kept) => column.width = kept.width,
                None => {
                    column.width = MIN_FITTED_WIDTH;
                    fresh.push(i);
                }
            }
        }
        let num_rows = match self.view() {
            Some(view) => view.num_rows(),
            None => document.num_rows_at(level).unwrap_or(0),
        };
        // Row numbers mean other rows now.
        if num_rows != self.num_rows {
            self.open_rows.clear();
        }
        self.num_rows = num_rows;
        let before = self.selected;
        self.selected = self.selected.and_then(|(r, c)| {
            (self.num_rows > 0 && !self.columns.is_empty())
                .then(|| (r.min(self.num_rows - 1), c.min(self.columns.len() - 1)))
        });
        if self.selected != before {
            self.item = None;
        }
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

    fn max_first_row(&self) -> usize {
        if self.details.is_empty() {
            return self.num_rows.saturating_sub(self.visible_rows);
        }
        // The first row from which the last rows fill the body.
        let mut first = self.num_rows;
        let mut lines = 0;
        while first > 0 {
            let more = self.row_lines(first - 1);
            if lines + more > self.visible_rows {
                break;
            }
            lines += more;
            first -= 1;
        }
        first.min(self.num_rows.saturating_sub(1))
    }

    /// Screen lines `row` takes: one, plus its items while open.
    fn row_lines(&self, row: usize) -> usize {
        self.details.get(&row).map_or(1, |columns| {
            let lines = columns
                .iter()
                .enumerate()
                .map(|(c, items)| items.lines(self.appending(row, c)))
                .max()
                .unwrap_or(0);
            1 + lines
        })
    }

    /// Whether the slot for a new item of cell (`row`, `column`) is selected.
    fn appending(&self, row: usize, column: usize) -> bool {
        self.selected == Some((row, column))
            && self.item.is_some()
            && self.item == self.items(row, column).map(|i| i.items.len())
    }

    /// The listed items of a cell of an open row.
    fn items(&self, row: usize, column: usize) -> Option<&Items> {
        self.details.get(&row)?.get(column)
    }

    /// Items that can be selected in cell (`row`, `column`): 0 unless it is
    /// an editable list in an open row.
    fn selectable_items(&self, row: usize, column: usize) -> usize {
        self.items(row, column)
            .map_or(0, |i| i.selectable(self.appending(row, column)))
    }

    /// Rows from `first` that fit fully in the body (at least one).
    fn rows_fitting(&self, first: usize) -> usize {
        let mut lines = 0;
        let mut rows = 0;
        for row in first..self.num_rows {
            lines += self.row_lines(row);
            if lines > self.visible_rows {
                break;
            }
            rows += 1;
        }
        rows.max(1)
    }

    /// The row at screen line `line` of the body, and the line within it.
    fn row_at_line(&self, line: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        for row in self.first_row..self.num_rows {
            let lines = self.row_lines(row);
            if line < start + lines {
                return Some((row, line - start));
            }
            start += lines;
        }
        None
    }

    /// First screen line of `row`; `None` above the view, and some line
    /// below the body for rows past it.
    fn line_of(&self, row: usize) -> Option<usize> {
        if row < self.first_row {
            return None;
        }
        let mut line = 0;
        for r in self.first_row..row {
            line += self.row_lines(r);
            if line > self.visible_rows + 1 {
                break;
            }
        }
        Some(line)
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
        } else if row >= first_row + self.rows_fitting(first_row) {
            // Make `row` the last row fully shown.
            let mut lines = self.row_lines(row);
            first_row = row;
            while first_row > 0 && lines + self.row_lines(first_row - 1) <= self.visible_rows {
                first_row -= 1;
                lines += self.row_lines(first_row);
            }
        }
        let Some(width) = self.columns.get(column).map(|c| c.width) else {
            // No columns (a new file with rows only): scroll rows only.
            self.set_scroll(first_row, self.scroll_x);
            return;
        };
        let left: f32 = self.columns[..column].iter().map(|c| c.width).sum();
        let right = left + width;
        let mut scroll_x = self.scroll_x;
        if left < scroll_x {
            scroll_x = left;
        } else if right > scroll_x + self.body_width {
            scroll_x = (right - self.body_width).min(left);
        }
        self.set_scroll(first_row, scroll_x);
    }

    /// Up and down through the items of open list cells. Returns whether
    /// it handled `nav`.
    fn navigate_items(&mut self, nav: Nav) -> bool {
        let Some((row, column)) = self.selected else {
            return false;
        };
        let count = self.selectable_items(row, column);
        match (nav, self.item) {
            (Nav::Down, None) if count > 0 => self.item = Some(0),
            (Nav::Down, Some(i)) if i + 1 < count => self.item = Some(i + 1),
            (Nav::Down, Some(_)) => {
                self.item = None;
                return false;
            }
            (Nav::Up, Some(0)) => self.item = None,
            (Nav::Up, Some(i)) => self.item = Some(i - 1),
            (Nav::Up, None) if row > 0 && self.selectable_items(row - 1, column) > 0 => {
                let items = self.items(row - 1, column).map_or(0, |i| i.items.len());
                self.selected = Some((row - 1, column));
                self.item = Some(items.saturating_sub(1));
            }
            _ => return false,
        }
        self.reveal(row, column);
        true
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
        let batch = match self.view() {
            Some(view) => view.read(start..end),
            None => document.read_at(self.level(document), start..end),
        };
        let formatted = batch.and_then(|batch| {
            let batch = self.display_batch(batch)?;
            let mut cells = format_batch(&batch)?;
            let mut details = std::collections::HashMap::new();
            for (i, column) in batch.columns().iter().enumerate() {
                if !has_items(column.data_type()) {
                    continue;
                }
                for (row, cells) in cells.iter_mut().enumerate() {
                    if let Some(summary) = veta_core::display::items_summary(column.as_ref(), row) {
                        cells[i] = Some(summary);
                    }
                }
            }
            for &row in self.open_rows.range(start..end) {
                let local = row - start;
                let columns = batch
                    .columns()
                    .iter()
                    .map(|column| {
                        let array = column.as_ref();
                        let kind = match array.data_type() {
                            DataType::List(_) | DataType::LargeList(_) => ItemsKind::List,
                            DataType::Struct(_) => ItemsKind::Struct,
                            t if has_items(t) => ItemsKind::Map,
                            _ => ItemsKind::None,
                        };
                        if kind == ItemsKind::Struct {
                            let items =
                                veta_core::display::struct_fields(array, local).unwrap_or_default();
                            return Items {
                                items,
                                more: 0,
                                kind,
                            };
                        }
                        let items = veta_core::display::list_items(array, local, MAX_ITEMS)
                            .unwrap_or_default();
                        let more = veta_core::display::item_count(array, local) - items.len();
                        Items { items, more, kind }
                    })
                    .collect();
                details.insert(row, columns);
            }
            Ok((cells, nested_json(&batch)?, details))
        });
        match formatted {
            Ok((cells, json, details)) => {
                self.window_start = start;
                self.cells = cells;
                self.json = json;
                self.details = details;
                self.error = None;
            }
            Err(e) => {
                self.cells.clear();
                self.json.clear();
                self.details.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    /// `batch` (document columns) as the columns on screen.
    fn display_batch(&self, batch: RecordBatch) -> veta_core::Result<RecordBatch> {
        if self.columns.iter().all(|c| c.path().is_empty()) {
            return Ok(batch);
        }
        let mut fields = Vec::new();
        let mut arrays = Vec::new();
        for (i, column) in self.columns.iter().enumerate() {
            let Some(array) = batch
                .columns()
                .get(column.source)
                .and_then(|a| veta_core::display::field_array(a, column.path()))
            else {
                // Out of step with the document; the next refresh fixes it.
                return Ok(batch);
            };
            fields.push(Field::new(format!("c{i}"), array.data_type().clone(), true));
            arrays.push(array);
        }
        let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
        Ok(RecordBatch::try_new_with_options(
            std::sync::Arc::new(Schema::new(fields)),
            arrays,
            &options,
        )?)
    }

    /// Whether a loaded list or map cell has items to open.
    fn has_items(&self, row: usize, column: usize) -> bool {
        // Editable lists open even when empty, to add items.
        self.columns
            .get(column)
            .is_some_and(|c| (c.list || c.is_struct) && self.value(row, column).is_some())
            || self.columns.get(column).is_some_and(|c| c.items)
                && matches!(self.value(row, column), Some(Some(v)) if v != "[]" && v != "{}")
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

/// A clicked cell: row, column and list item.
type ClickedCell = (usize, usize, Option<usize>);

#[derive(Debug, Default)]
struct State {
    focused: bool,
    drag: Drag,
    reported: Option<(usize, u32)>,
    /// Time and cell of the last click, to detect double-clicks.
    last_click: Option<(Instant, ClickedCell)>,
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
        let line = self.view.line_of(row)?;
        let (_, x, width) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(c, _, _)| c == column)?;
        // An item: its line, after the item labels' indent.
        let (line, indent) = match self.view.item {
            Some(item) => (line + 1 + item, regions.m.text_size + 2.0),
            None => (line, 0.0),
        };
        let rect = Rectangle {
            x: x + indent,
            y: regions.body.y + line as f32 * regions.m.row_height,
            width: width - indent,
            height: regions.m.row_height,
        };
        rect.intersection(&regions.body)
    }

    /// Row under a y coordinate in the body.
    fn row_at(&self, regions: &Regions, y: f32) -> Option<usize> {
        if y < regions.body.y || y >= regions.body.y + regions.body.height {
            return None;
        }
        let line = ((y - regions.body.y) / regions.m.row_height) as usize;
        self.view.row_at_line(line).map(|(row, _)| row)
    }

    /// The row whose item toggle (in a list or map cell) is under
    /// `position`.
    fn items_toggle_at(&self, regions: &Regions, position: Point) -> Option<usize> {
        if !regions.body.contains(position) {
            return None;
        }
        let line = ((position.y - regions.body.y) / regions.m.row_height) as usize;
        let (row, 0) = self.view.row_at_line(line)? else {
            return None;
        };
        let (column, x, _) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(_, left, w)| position.x >= left && position.x < left + w)?;
        let on_toggle = position.x < x + CELL_PADDING + regions.m.text_size + 2.0;
        (on_toggle && self.view.has_items(row, column)).then_some(row)
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

    /// Expand or collapse buttons of a header, left to right, with their x.
    fn toggles(&self, column: usize, x: f32) -> Vec<(GridEvent, Icon, f32)> {
        let c = &self.view.columns[column];
        let step = self.view.metrics.text_size + 2.0;
        let mut x = x + CELL_PADDING;
        let mut out = Vec::new();
        if c.collapses.is_some() {
            out.push((GridEvent::Collapse { column }, Icon::ChevronDown, x));
            x += step;
        }
        if c.expandable {
            out.push((GridEvent::Expand { column }, Icon::ChevronRight, x));
        }
        out
    }

    /// The expand or collapse button under `position` in the header.
    fn toggle_at(&self, regions: &Regions, position: Point) -> Option<GridEvent> {
        if position.y > regions.header.y + regions.m.header_height / 2.0 + 3.0 {
            return None;
        }
        let (column, x, _) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(_, left, w)| position.x >= left && position.x < left + w)?;
        let step = regions.m.text_size + 2.0;
        self.toggles(column, x)
            .into_iter()
            .find(|&(_, _, left)| position.x >= left - 2.0 && position.x < left + step)
            .map(|(event, _, _)| event)
    }

    /// Column under an x coordinate.
    fn column_at(&self, regions: &Regions, x: f32) -> Option<usize> {
        self.visible_columns(regions.body)
            .into_iter()
            .find(|&(_, left, w)| x >= left && x < left + w)
            .map(|(i, _, _)| i)
    }

    /// The item of an open list cell under `position`: (row, column, item).
    fn item_at(&self, regions: &Regions, position: Point) -> Option<(usize, usize, usize)> {
        if !regions.body.contains(position) {
            return None;
        }
        let line = ((position.y - regions.body.y) / regions.m.row_height) as usize;
        let (row, sub) = self.view.row_at_line(line)?;
        let item = sub.checked_sub(1)?;
        let (column, _, _) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(_, x, w)| position.x >= x && position.x < x + w)?;
        (item < self.view.selectable_items(row, column)).then_some((row, column, item))
    }

    fn cell_at(&self, regions: &Regions, position: Point) -> Option<(usize, usize)> {
        if !regions.body.contains(position) {
            return None;
        }
        let line = ((position.y - regions.body.y) / regions.m.row_height) as usize;
        let (row, _) = self.view.row_at_line(line)?;
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
                    if let Some(event) = self.toggle_at(&regions, position) {
                        shell.publish((self.on_event)(event));
                    } else if let Some(column) = self.column_at(&regions, position.x) {
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
                    let item = self.item_at(&regions, position).map(|(_, _, i)| i);
                    match item {
                        Some(item) => {
                            shell.publish((self.on_event)(GridEvent::SelectItem {
                                row,
                                column,
                                item,
                            }));
                        }
                        None => shell.publish((self.on_event)(GridEvent::Select { row, column })),
                    }
                    if item.is_none() && self.items_toggle_at(&regions, position) == Some(row) {
                        shell.publish((self.on_event)(GridEvent::ToggleItems { row }));
                        state.last_click = None;
                        return;
                    }
                    let now = Instant::now();
                    let double = state.last_click.is_some_and(|(at, cell)| {
                        cell == (row, column, item) && now.duration_since(at) < DOUBLE_CLICK
                    });
                    if double {
                        state.last_click = None;
                        shell.publish((self.on_event)(GridEvent::StartEdit { initial: None }));
                    } else {
                        state.last_click = Some((now, (row, column, item)));
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
                let target = if let Some((row, column, item)) = self.item_at(&regions, position) {
                    shell.publish((self.on_event)(GridEvent::SelectItem { row, column, item }));
                    if view.columns[column].is_struct {
                        Some(MenuTarget::Field)
                    } else {
                        Some(MenuTarget::Item)
                    }
                } else if let Some((row, column)) = self.cell_at(&regions, position) {
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
                    self.column_at(&regions, position.x).map(|column| {
                        let selected = view
                            .selected_display_columns()
                            .is_some_and(|c| c.contains(&column));
                        if !selected {
                            shell.publish((self.on_event)(GridEvent::SelectColumns {
                                column,
                                extend: false,
                            }));
                        }
                        MenuTarget::Column(view.columns[column].source)
                    })
                } else {
                    Some(MenuTarget::Blank)
                };
                if let Some(target) = target {
                    shell.publish((self.on_event)(GridEvent::ContextMenu { target, position }));
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            // A press that started elsewhere (e.g. in a file dialog) must not
            // leave a drag running: its release never reaches the grid.
            Event::Window(iced::window::Event::Focused | iced::window::Event::Unfocused) => {
                state.drag = Drag::None;
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
                    // Fields of an expanded struct move with their column.
                    let slot = self.drop_slot(&regions, x);
                    let from = view.columns[column].source;
                    let slot = view
                        .columns
                        .get(slot)
                        .map_or(view.columns.last().map_or(0, |c| c.source + 1), |c| {
                            c.source
                        });
                    let to = if slot > from { slot - 1 } else { slot };
                    if to != from {
                        shell.publish((self.on_event)(GridEvent::MoveColumn { from, to }));
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
        // Rows on screen with their first line and line count.
        let lines_on_screen = (regions.body.height / m.row_height).ceil() as usize;
        let mut rows_shown = Vec::new();
        let mut next_line = 0;
        for row in view.first_row..view.num_rows {
            if next_line >= lines_on_screen {
                break;
            }
            let lines = view.row_lines(row);
            rows_shown.push((row, next_line, lines));
            next_line += lines;
        }
        let row_y = |line: usize| regions.body.y + line as f32 * m.row_height;

        // Body: stripes, selection, cells.
        for &(row, line, lines) in &rows_shown {
            let y = row_y(line);
            let row_rect = Rectangle {
                x: regions.body.x,
                y,
                width: regions.body.width,
                height: lines as f32 * m.row_height,
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
                let mut text_cell = cell;
                if view.has_items(row, column) {
                    let open = view.open_rows.contains(&row);
                    let glyph = if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    };
                    draw_text(
                        renderer,
                        glyph.codepoint().to_string(),
                        cell,
                        false,
                        m.text_size,
                        icon::FONT,
                        t.selection,
                        clip,
                    );
                    text_cell.x += m.text_size + 2.0;
                }
                draw_text(
                    renderer,
                    content,
                    text_cell,
                    numeric,
                    m.text_size,
                    Font::DEFAULT,
                    color,
                    clip,
                );
                // Items of an open row, one per line: index or key, value.
                let Some(items) = view.details.get(&row).and_then(|d| d.get(column)) else {
                    continue;
                };
                let indent = m.text_size + 2.0;
                for (k, item) in items.items.iter().enumerate() {
                    let (label, value) = (&item.label, &item.text);
                    let line_cell = Rectangle {
                        x: x + indent,
                        y: y + (k + 1) as f32 * m.row_height,
                        width: width - indent,
                        height: m.row_height,
                    };
                    let Some(clip) = line_cell.intersection(&regions.body) else {
                        continue;
                    };
                    draw_text(
                        renderer,
                        label.clone(),
                        line_cell,
                        false,
                        m.type_text_size,
                        Font::DEFAULT,
                        muted,
                        clip,
                    );
                    let label_width = (label.chars().count() + 1) as f32 * m.char_width;
                    let value_cell = Rectangle {
                        x: line_cell.x + label_width,
                        width: line_cell.width - label_width,
                        ..line_cell
                    };
                    let (text, color) = match value {
                        Some(v) => (single_line(v), text_color),
                        None => (NULL_TEXT.to_owned(), muted),
                    };
                    draw_text(
                        renderer,
                        text,
                        value_cell,
                        false,
                        m.text_size,
                        Font::DEFAULT,
                        color,
                        clip,
                    );
                }
                // The slot for a new item.
                if items.has_slot(view.appending(row, column)) {
                    let line_cell = Rectangle {
                        x: x + indent,
                        y: y + (items.items.len() + 1) as f32 * m.row_height,
                        width: width - indent,
                        height: m.row_height,
                    };
                    if let Some(clip) = line_cell.intersection(&regions.body) {
                        let text = if items.items.is_empty() {
                            "empty".to_owned()
                        } else {
                            items.items.len().to_string()
                        };
                        draw_text(
                            renderer,
                            text,
                            line_cell,
                            false,
                            m.type_text_size,
                            Font::DEFAULT,
                            muted,
                            clip,
                        );
                    }
                }
                if items.more > 0 {
                    let line_cell = Rectangle {
                        x: x + indent,
                        y: y + (items.items.len() + 1) as f32 * m.row_height,
                        width: width - indent,
                        height: m.row_height,
                    };
                    if let Some(clip) = line_cell.intersection(&regions.body) {
                        draw_text(
                            renderer,
                            format!("… {} more", items.more),
                            line_cell,
                            false,
                            m.text_size,
                            Font::DEFAULT,
                            muted,
                            clip,
                        );
                    }
                }
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
            && let Some(&(_, line, _)) = rows_shown.iter().find(|(r, _, _)| *r == row)
            && let Some(&(_, x, width)) = columns.iter().find(|(c, _, _)| *c == column)
        {
            let (line, indent) = match view.item {
                Some(item) => (line + 1 + item, m.text_size + 2.0),
                None => (line, 0.0),
            };
            let rect = Rectangle {
                x: x + indent,
                width: width - indent,
                y: row_y(line),
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
            view.selected_display_columns().unwrap_or(0..0)
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
            let toggles = self.toggles(column, x);
            for (_, glyph, left) in &toggles {
                let button = Rectangle {
                    x: left - CELL_PADDING,
                    ..cell
                };
                draw_text(
                    renderer,
                    glyph.codepoint().to_string(),
                    button,
                    false,
                    m.text_size,
                    icon::FONT,
                    t.selection,
                    clip,
                );
            }
            let indent = toggles.len() as f32 * (m.text_size + 2.0);
            let name_cell = Rectangle {
                x: cell.x + indent,
                ..cell
            };
            draw_text(
                renderer,
                c.name.clone(),
                name_cell,
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
        for &(row, line, _) in &rows_shown {
            let cell = Rectangle {
                x: regions.row_numbers.x,
                y: row_y(line),
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
                "No columns yet. Right-click here to add one.".into(),
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

/// JSON text of the nested columns of `batch`: `result[row][column]` is
/// `None` for columns that aren't nested.
fn nested_json(
    batch: &veta_core::arrow::record_batch::RecordBatch,
) -> veta_core::Result<Vec<Vec<Option<Option<String>>>>> {
    let nested: Vec<bool> = batch
        .columns()
        .iter()
        .map(|c| c.data_type().is_nested())
        .collect();
    if !nested.contains(&true) {
        return Ok(Vec::new());
    }
    (0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .zip(&nested)
                .map(|(column, &is_nested)| {
                    if is_nested {
                        Ok(Some(veta_core::display::value_json(column.as_ref(), row)?))
                    } else {
                        Ok(None)
                    }
                })
                .collect()
        })
        .collect()
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
