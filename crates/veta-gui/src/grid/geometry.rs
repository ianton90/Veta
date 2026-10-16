//! Column layout and hit testing: the columns on screen, row geometry,
//! scrollbars and which cell, header or border is under the cursor.

use super::*;

/// A column on screen: a document column, or a field of an expanded struct
/// column.
#[derive(Debug, Clone)]
pub(super) struct Column {
    /// The document column's name followed by the field names down to this
    /// column.
    pub(super) key: Vec<String>,
    /// Index of the document column.
    pub(super) source: usize,
    pub(super) name: String,
    pub(super) data_type: DataType,
    pub(super) type_name: String,
    pub(super) numeric: bool,
    pub(super) width: f32,
    /// A struct whose fields aren't shown.
    pub(super) expandable: bool,
    /// Structs, lists and maps: cells open into lines (see `items`).
    pub(super) items: bool,
    /// The expanded struct whose first field this is (collapse button).
    pub(super) collapses: Option<Vec<String>>,
}

impl Column {
    /// Field names below the document column.
    pub(super) fn path(&self) -> &[String] {
        &self.key[1..]
    }
}

/// The columns on screen for `schema`, with the structs in `expanded` shown
/// as their fields.
pub(super) fn display_columns(schema: &Schema, expanded: &BTreeSet<Vec<String>>) -> Vec<Column> {
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
            items: veta_core::values::node::NodeKind::of(data_type).is_container(),
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

impl GridView {
    /// Sizes each column to its header and the values currently loaded.
    pub(super) fn fit_columns(&mut self) {
        for i in 0..self.columns.len() {
            self.fit_column(i);
        }
    }

    pub(super) fn fit_column(&mut self, i: usize) {
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

    pub(super) fn max_first_row(&self) -> usize {
        if self.trees.is_empty() {
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

    /// Screen lines `row` takes: one, plus the lines of its cells while
    /// open.
    pub(super) fn row_lines(&self, row: usize) -> usize {
        self.trees.get(&row).map_or(1, |cells| {
            1 + cells.iter().map(|t| t.lines.len()).max().unwrap_or(0)
        })
    }

    /// Rows from `first` that fit fully in the body (at least one).
    pub(super) fn rows_fitting(&self, first: usize) -> usize {
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
    pub(super) fn row_at_line(&self, line: usize) -> Option<(usize, usize)> {
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

    pub(super) fn content_width(&self) -> f32 {
        self.columns.iter().map(|c| c.width).sum()
    }

    pub(super) fn max_scroll_x(&self) -> f32 {
        (self.content_width() - self.body_width).max(0.0)
    }

    pub(super) fn set_scroll(&mut self, first_row: usize, scroll_x: f32) {
        self.first_row = first_row.min(self.max_first_row());
        self.scroll_x = scroll_x.clamp(0.0, self.max_scroll_x());
    }

    /// Scrolls the minimum needed for a cell to be fully visible.
    pub(super) fn reveal(&mut self, row: usize, column: usize) {
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

    fn row_number_width(&self) -> f32 {
        let digits = self.num_rows.max(1).to_string().len() as f32;
        digits * self.metrics.char_width + 3.0 * CELL_PADDING
    }
}

/// Screen regions of the grid, computed from its bounds.
pub(super) struct Regions {
    pub(super) m: Metrics,
    pub(super) bounds: Rectangle,
    pub(super) header: Rectangle,
    pub(super) row_numbers: Rectangle,
    pub(super) body: Rectangle,
    pub(super) v_track: Rectangle,
    pub(super) h_track: Rectangle,
}

impl Regions {
    pub(super) fn new(bounds: Rectangle, view: &GridView) -> Self {
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

    pub(super) fn full_rows(&self) -> usize {
        (self.body.height / self.m.row_height).floor() as usize
    }

    /// Vertical thumb (offset from track top, length).
    pub(super) fn v_thumb(&self, view: &GridView) -> Option<(f32, f32)> {
        thumb(
            self.v_track.height,
            view.num_rows as f32,
            view.visible_rows as f32,
            view.first_row as f32,
        )
    }

    pub(super) fn h_thumb(&self, view: &GridView) -> Option<(f32, f32)> {
        thumb(
            self.h_track.width,
            view.content_width(),
            self.body.width,
            view.scroll_x,
        )
    }
}

/// Thumb position and length for a scrollbar, or `None` if everything fits.
pub(super) fn thumb(track: f32, total: f32, visible: f32, offset: f32) -> Option<(f32, f32)> {
    if total <= visible || track <= 0.0 {
        return None;
    }
    let length = (track * visible / total).clamp(MIN_THUMB.min(track), track);
    let max_offset = total - visible;
    let position = (track - length) * (offset / max_offset).clamp(0.0, 1.0);
    Some((position, length))
}

/// Inverse of [`thumb`]: offset for a thumb at `position` in the track.
pub(super) fn offset_for_thumb(
    track: f32,
    length: f32,
    total: f32,
    visible: f32,
    position: f32,
) -> f32 {
    let free = track - length;
    if free <= 0.0 {
        return 0.0;
    }
    (position / free).clamp(0.0, 1.0) * (total - visible).max(0.0)
}

impl<Message> Grid<'_, Message> {
    /// Column index and its x range on screen, for columns intersecting the body.
    pub(super) fn visible_columns(&self, body: Rectangle) -> Vec<(usize, f32, f32)> {
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

    pub(super) fn column_border_at(&self, regions: &Regions, position: Point) -> Option<usize> {
        if !regions.header.contains(position) {
            return None;
        }
        self.visible_columns(regions.body)
            .into_iter()
            .find(|&(_, x, w)| (position.x - (x + w)).abs() <= RESIZE_GRAB)
            .map(|(i, _, _)| i)
    }

    /// Screen rectangle of the selected cell, if it is visible.
    pub(super) fn selected_cell_rect(&self, regions: &Regions) -> Option<Rectangle> {
        let (row, column) = self.view.selected?;
        let line = self.view.line_of(row)?;
        let (_, x, width) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(c, _, _)| c == column)?;
        // A line of an open cell: from its arrow (or label) on.
        let (line, indent) = match (self.view.item, self.view.selected_line()) {
            (Some(item), Some(l)) => (line + 1 + item, line_offsets(regions.m, l).0),
            _ => (line, 0.0),
        };
        let rect = Rectangle {
            x: x + indent,
            y: regions.body.y + line as f32 * regions.m.row_height,
            width: width - indent,
            height: regions.m.row_height,
        };
        rect.intersection(&regions.body)
    }

    /// Where the editor goes: the selected cell, or for an item the part
    /// after its label (index, key or field name), unless the editor covers
    /// the label.
    pub(super) fn editor_rect(&self, regions: &Regions) -> Option<Rectangle> {
        let rect = self.selected_cell_rect(regions)?;
        let Some(line) = self.view.item.and(self.view.selected_line()) else {
            return Some(rect);
        };
        // As the line is drawn (see `draw`).
        let (arrow, label) = line_offsets(regions.m, line);
        let mut shift = label - arrow;
        if !self.editor_covers_label {
            shift += label_width(regions.m, &line.label);
        }
        let shift = shift.min(rect.width / 2.0);
        Some(Rectangle {
            x: rect.x + shift,
            width: rect.width - shift,
            ..rect
        })
    }

    /// Row under a y coordinate in the body.
    pub(super) fn row_at(&self, regions: &Regions, y: f32) -> Option<usize> {
        if y < regions.body.y || y >= regions.body.y + regions.body.height {
            return None;
        }
        let line = ((y - regions.body.y) / regions.m.row_height) as usize;
        self.view.row_at_line(line).map(|(row, _)| row)
    }

    /// The row whose item toggle (in a list or map cell) is under
    /// `position`.
    pub(super) fn items_toggle_at(&self, regions: &Regions, position: Point) -> Option<usize> {
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
    pub(super) fn drop_slot(&self, regions: &Regions, x: f32) -> usize {
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
    pub(super) fn slot_x(&self, regions: &Regions, slot: usize) -> f32 {
        regions.body.x - self.view.scroll_x
            + self.view.columns[..slot]
                .iter()
                .map(|c| c.width)
                .sum::<f32>()
    }

    /// Expand or collapse buttons of a header, left to right, with their x.
    pub(super) fn toggles(&self, column: usize, x: f32) -> Vec<(GridEvent, Icon, f32)> {
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
    pub(super) fn toggle_at(&self, regions: &Regions, position: Point) -> Option<GridEvent> {
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
    pub(super) fn column_at(&self, regions: &Regions, x: f32) -> Option<usize> {
        self.visible_columns(regions.body)
            .into_iter()
            .find(|&(_, left, w)| x >= left && x < left + w)
            .map(|(i, _, _)| i)
    }

    /// The item of an open list cell under `position`: (row, column, item).
    pub(super) fn item_at(
        &self,
        regions: &Regions,
        position: Point,
    ) -> Option<(usize, usize, usize)> {
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
        self.view
            .is_selectable(row, column, item)
            .then_some((row, column, item))
    }

    /// Whether `position` is on the open/close arrow of a line of an open
    /// cell (a struct, list or map).
    pub(super) fn line_toggle_at(&self, regions: &Regions, position: Point) -> bool {
        let Some((row, column, item)) = self.item_at(regions, position) else {
            return false;
        };
        let Some(line) = self.view.tree(row, column).and_then(|t| t.lines.get(item)) else {
            return false;
        };
        let What::Node { kind, .. } = line.what else {
            return false;
        };
        let Some((_, x, _)) = self
            .visible_columns(regions.body)
            .into_iter()
            .find(|&(c, _, _)| c == column)
        else {
            return false;
        };
        let arrow = x + line_offsets(regions.m, line).0;
        kind.is_container()
            && position.x >= arrow - 2.0
            && position.x < arrow + regions.m.text_size + 2.0
    }

    pub(super) fn cell_at(&self, regions: &Regions, position: Point) -> Option<(usize, usize)> {
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

/// Where a line of an open cell starts its arrow and its label, from the
/// cell's left edge.
pub(super) fn line_offsets(m: Metrics, line: &Line) -> (f32, f32) {
    let arrow = m.text_size + 2.0 + (line.depth - 1) as f32 * indent_step(m);
    let label = arrow + if line.gutter { m.text_size + 2.0 } else { 0.0 };
    (arrow, label)
}

/// Width taken by a line's label and the space after it.
pub(super) fn label_width(m: Metrics, label: &str) -> f32 {
    if label.is_empty() {
        return 0.0;
    }
    (label.chars().count() + 1) as f32 * m.char_width
}

/// How much each level of an open cell is indented.
pub(super) fn indent_step(m: Metrics) -> f32 {
    (m.text_size * 1.6).round()
}
