//! Drawing the grid.

use super::*;
use veta_core::values::node::NodeKind;

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
                    // Still being read.
                    None => ("…".to_owned(), muted),
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
                // Lines of an open cell: arrow, label (field, index or
                // key) and value, indented by depth.
                let Some(tree) = view.tree(row, column) else {
                    continue;
                };
                for (k, line) in tree.lines.iter().enumerate() {
                    let line_cell = Rectangle {
                        x,
                        y: y + (k + 1) as f32 * m.row_height,
                        width,
                        height: m.row_height,
                    };
                    let Some(clip) = line_cell.intersection(&regions.body) else {
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
                        x: x + offset,
                        width: (width - offset).max(0.0),
                        ..line_cell
                    };
                    let muted_text =
                        |renderer: &mut Renderer, text: String, offset: f32, size: f32| {
                            draw_text(
                                renderer,
                                text,
                                at(offset),
                                false,
                                size,
                                Font::DEFAULT,
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
                                Some(v) => (single_line(v), text_color),
                                None => (NULL_TEXT.to_owned(), muted),
                            };
                            draw_text(
                                renderer,
                                text,
                                at(label + label_width(m, &line.label)),
                                false,
                                m.text_size,
                                Font::DEFAULT,
                                color,
                                clip,
                            );
                        }
                        What::Slot(place) => {
                            let text = match (place.len, place.parent) {
                                (0, _) => "empty".to_owned(),
                                (_, NodeKind::Map) => "new entry".to_owned(),
                                (n, _) => n.to_string(),
                            };
                            muted_text(renderer, text, label, m.type_text_size);
                        }
                        What::More(n) => {
                            muted_text(renderer, format!("… {n} more"), label, m.text_size);
                        }
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
