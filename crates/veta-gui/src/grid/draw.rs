//! Drawing the grid.

use super::*;
use veta_core::values::node::NodeKind;

/// What one frame draws: the regions, and the columns and rows on screen.
struct Frame {
    regions: Regions,
    m: Metrics,
    /// Columns on screen: index, left edge and width.
    columns: Vec<(usize, f32, f32)>,
    /// Rows on screen: row, first line and line count.
    rows: Vec<(usize, usize, usize)>,
}

impl Frame {
    /// Top of screen line `line` of the body.
    fn row_y(&self, line: usize) -> f32 {
        self.regions.body.y + line as f32 * self.m.row_height
    }
}

/// A cell's box, and the part of it that is on screen.
struct Cell {
    rect: Rectangle,
    clip: Rectangle,
}

impl<Message> Grid<'_, Message> {
    /// Body of [`Widget::draw`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_grid(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let frame = self.frame(layout.bounds());
        let regions = &frame.regions;
        fill(renderer, regions.bounds, self.tokens.background);
        self.draw_body(renderer, &frame);
        self.draw_column_lines(renderer, &frame);
        self.draw_selection(renderer, &frame);
        self.draw_header(renderer, &frame);
        self.draw_row_numbers(renderer, &frame);
        self.draw_scrollbars(renderer, regions);

        let state = tree.state.downcast_ref::<State>();
        if let Drag::Header {
            x, moving: true, ..
        } = state.drag
        {
            // Where the dragged column would go.
            let line_x = self.slot_x(regions, self.drop_slot(regions, x));
            fill(
                renderer,
                Rectangle {
                    x: line_x - 1.0,
                    y: regions.bounds.y,
                    width: 3.0,
                    height: frame.m.header_height + regions.body.height,
                },
                self.tokens.selection,
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

        if self.view.columns.is_empty() {
            draw_text(
                renderer,
                tr("ST_GRID_NO_COLUMNS"),
                regions.body,
                false,
                frame.m.text_size,
                crate::fonts::font(),
                self.tokens.muted_text,
                regions.body,
            );
        }
    }

    fn frame(&self, bounds: Rectangle) -> Frame {
        let view = self.view;
        let regions = Regions::new(bounds, view);
        let m = regions.m;
        let columns = self.visible_columns(regions.body);
        let lines_on_screen = (regions.body.height / m.row_height).ceil() as usize;
        let mut rows = Vec::new();
        let mut next_line = 0;
        for row in view.first_row..view.num_rows {
            if next_line >= lines_on_screen {
                break;
            }
            let lines = view.row_lines(row);
            rows.push((row, next_line, lines));
            next_line += lines;
        }
        Frame {
            regions,
            m,
            columns,
            rows,
        }
    }

    /// Stripes, selected rows and cells.
    fn draw_body(&self, renderer: &mut Renderer, frame: &Frame) {
        let (view, t, body) = (self.view, &self.tokens, frame.regions.body);
        // Whole-row selection only; a single selected cell is outlined.
        let selected_rows = if view.rows.is_some() {
            view.selected_rows().unwrap_or(0..0)
        } else {
            0..0
        };
        for &(row, line, lines) in &frame.rows {
            let y = frame.row_y(line);
            let row_rect = Rectangle {
                x: body.x,
                y,
                width: body.width,
                height: lines as f32 * frame.m.row_height,
            };
            let Some(row_rect) = row_rect.intersection(&body) else {
                continue;
            };
            if row % 2 == 1 {
                fill(renderer, row_rect, t.grid_stripe);
            }
            if selected_rows.contains(&row) {
                let highlight = Color {
                    a: 0.14,
                    ..t.selection
                };
                fill(renderer, row_rect, highlight);
            }
            for &(column, x, width) in &frame.columns {
                let rect = Rectangle {
                    x,
                    y,
                    width,
                    height: frame.m.row_height,
                };
                let Some(clip) = rect.intersection(&body) else {
                    continue;
                };
                self.draw_cell(renderer, frame, row, column, Cell { rect, clip });
                if let Some(tree) = view.tree(row, column) {
                    self.draw_tree_lines(renderer, frame, tree, rect);
                }
            }
        }
    }

    /// A cell's value, with an arrow if it opens into lines.
    fn draw_cell(
        &self,
        renderer: &mut Renderer,
        frame: &Frame,
        row: usize,
        column: usize,
        cell: Cell,
    ) {
        let (view, t, m) = (self.view, &self.tokens, frame.m);
        let (content, color) = match view.cell(row, column) {
            Some(Some(value)) => (single_line(value), t.text),
            Some(None) => (NULL_TEXT.to_owned(), t.muted_text),
            // Still being read.
            None => ("…".to_owned(), t.muted_text),
        };
        let mut text_cell = cell.rect;
        if view.has_items(row, column) {
            let glyph = if view.open_rows.contains(&row) {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            draw_text(
                renderer,
                glyph.codepoint().to_string(),
                cell.rect,
                false,
                m.text_size,
                icon::FONT,
                t.selection,
                cell.clip,
            );
            text_cell.x += m.text_size + 2.0;
        }
        draw_text(
            renderer,
            content,
            text_cell,
            view.columns[column].numeric,
            m.text_size,
            crate::fonts::font(),
            color,
            cell.clip,
        );
    }

    /// Lines of an open cell below its first line: arrow, label (field,
    /// index or key) and value, indented by depth.
    fn draw_tree_lines(
        &self,
        renderer: &mut Renderer,
        frame: &Frame,
        tree: &CellTree,
        cell: Rectangle,
    ) {
        let (t, m) = (&self.tokens, frame.m);
        let muted = t.muted_text;
        for (k, line) in tree.lines.iter().enumerate() {
            let line_cell = Rectangle {
                y: cell.y + (k + 1) as f32 * m.row_height,
                ..cell
            };
            let Some(clip) = line_cell.intersection(&frame.regions.body) else {
                continue;
            };
            // Levels alternate between two shades.
            let shade = if line.depth % 2 == 1 {
                t.nested_odd
            } else {
                t.nested_even
            };
            fill(renderer, clip, shade);
            let (arrow, label) = line_offsets(m, line);
            let at = |offset: f32| Rectangle {
                x: cell.x + offset,
                width: (cell.width - offset).max(0.0),
                ..line_cell
            };
            let muted_text = |renderer: &mut Renderer, text: String, offset: f32, size: f32| {
                draw_text(
                    renderer,
                    text,
                    at(offset),
                    false,
                    size,
                    crate::fonts::font(),
                    muted,
                    clip,
                );
            };
            match &line.what {
                What::Node { kind, open, .. } => {
                    if kind.is_container() {
                        let glyph = if *open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        };
                        draw_text(
                            renderer,
                            glyph.codepoint().to_string(),
                            at(arrow),
                            false,
                            m.text_size,
                            icon::FONT,
                            t.selection,
                            clip,
                        );
                    }
                    muted_text(renderer, line.label.clone(), label, m.type_text_size);
                    let (text, color) = match &line.text {
                        Some(v) => (single_line(v), t.text),
                        None => (NULL_TEXT.to_owned(), muted),
                    };
                    draw_text(
                        renderer,
                        text,
                        at(label + label_width(m, &line.label)),
                        false,
                        m.text_size,
                        crate::fonts::font(),
                        color,
                        clip,
                    );
                }
                What::Slot(place) => {
                    let text = match (place.len, place.parent) {
                        (0, _) => tr("ST_GRID_EMPTY"),
                        (_, NodeKind::Map) => tr("ST_GRID_NEW_ENTRY"),
                        (n, _) => n.to_string(),
                    };
                    muted_text(renderer, text, label, m.type_text_size);
                }
                What::More(n) => {
                    muted_text(
                        renderer,
                        tr_with("ST_GRID_MORE", &[("n", &n)]),
                        label,
                        m.text_size,
                    );
                }
            }
        }
    }

    /// Vertical grid lines in the body and header.
    fn draw_column_lines(&self, renderer: &mut Renderer, frame: &Frame) {
        let (regions, m) = (&frame.regions, frame.m);
        for &(_, x, width) in &frame.columns {
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
                    self.tokens.border,
                );
            }
        }
    }

    /// The outline of the selected cell, or of the selected line in it.
    fn draw_selection(&self, renderer: &mut Renderer, frame: &Frame) {
        let (view, m) = (self.view, frame.m);
        let Some((row, column)) = view.selected else {
            return;
        };
        let (Some(&(_, line, _)), Some(&(_, x, width))) = (
            frame.rows.iter().find(|(r, _, _)| *r == row),
            frame.columns.iter().find(|(c, _, _)| *c == column),
        ) else {
            return;
        };
        let (line, indent) = match view.item {
            Some(item) => (line + 1 + item, m.text_size + 2.0),
            None => (line, 0.0),
        };
        let rect = Rectangle {
            x: x + indent,
            width: width - indent,
            y: frame.row_y(line),
            height: m.row_height,
        };
        if let Some(clip) = rect.intersection(&frame.regions.body) {
            renderer.fill_quad(
                Quad {
                    bounds: clip,
                    border: Border {
                        color: self.tokens.selection,
                        width: 2.0,
                        radius: 0.0.into(),
                    },
                    ..Quad::default()
                },
                Color::TRANSPARENT,
            );
        }
    }

    /// Column names and types, their toggles, and selected columns.
    fn draw_header(&self, renderer: &mut Renderer, frame: &Frame) {
        let (view, t, regions, m) = (self.view, &self.tokens, &frame.regions, frame.m);
        fill(renderer, regions.header, t.grid_header);
        let selected_cols = if view.cols.is_some() {
            view.selected_display_columns().unwrap_or(0..0)
        } else {
            0..0
        };
        let highlight = Color {
            a: 0.14,
            ..t.selection
        };
        for &(column, x, width) in &frame.columns {
            if selected_cols.contains(&column) {
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
        for &(column, x, width) in &frame.columns {
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
                crate::fonts::bold(),
                t.text,
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
                    crate::fonts::font(),
                    t.muted_text,
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
            t.muted_text,
        );
    }

    /// Row numbers down the left, selected ones highlighted.
    fn draw_row_numbers(&self, renderer: &mut Renderer, frame: &Frame) {
        let (t, regions, m) = (&self.tokens, &frame.regions, frame.m);
        fill(renderer, regions.row_numbers, t.grid_header);
        let corner = Rectangle {
            x: regions.bounds.x,
            y: regions.bounds.y,
            width: regions.row_numbers.width,
            height: m.header_height - 1.0,
        };
        fill(renderer, corner, t.grid_header);
        let highlighted = self.view.selected_rows().unwrap_or(0..0);
        for &(row, line, _) in &frame.rows {
            let cell = Rectangle {
                x: regions.row_numbers.x,
                y: frame.row_y(line),
                width: regions.row_numbers.width - CELL_PADDING / 2.0,
                height: m.row_height,
            };
            let Some(clip) = cell.intersection(&regions.row_numbers) else {
                continue;
            };
            let (font, color) = if highlighted.contains(&row) {
                (crate::fonts::bold(), t.selection)
            } else {
                (crate::fonts::font(), t.muted_text)
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
    }

    fn draw_scrollbars(&self, renderer: &mut Renderer, regions: &Regions) {
        let view = self.view;
        let track = self.tokens.grid_header;
        let thumb_color = self.tokens.border;
        fill(renderer, regions.v_track, track);
        fill(renderer, regions.h_track, track);
        let corner = Rectangle {
            x: regions.v_track.x,
            y: regions.h_track.y,
            width: SCROLLBAR,
            height: SCROLLBAR,
        };
        fill(renderer, corner, track);
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
