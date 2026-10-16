//! The [`Grid`] widget: layout, input handling and focus.

use super::*;
use veta_core::values::node::NodeKind;

/// The grid widget. Create with [`grid`].
pub struct Grid<'a, Message> {
    pub(super) view: &'a GridView,
    pub(super) tokens: Tokens,
    pub(super) on_event: Box<dyn Fn(GridEvent) -> Message + 'a>,
    /// Editor drawn over the selected cell while it is being edited.
    pub(super) editor: Option<Element<'a, Message>>,
    /// The editor covers the selected item's label (a map key being edited)
    /// instead of starting after it.
    pub(super) editor_covers_label: bool,
}

impl<'a, Message> Grid<'a, Message> {
    /// Shows `editor` over the selected cell.
    pub fn editor(mut self, editor: Option<Element<'a, Message>>) -> Self {
        self.editor = editor;
        self
    }

    /// Lays the editor over the selected item's label too (to edit a map
    /// key).
    pub fn editor_covers_label(mut self, covers: bool) -> Self {
        self.editor_covers_label = covers;
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
        editor_covers_label: false,
    }
}

/// A clicked cell: row, column and list item.
type ClickedCell = (usize, usize, Option<usize>);

#[derive(Debug, Default)]
pub(super) struct State {
    pub(super) focused: bool,
    pub(super) drag: Drag,
    pub(super) reported: Option<(usize, u32)>,
    /// Time and cell of the last click, to detect double-clicks.
    pub(super) last_click: Option<(Instant, ClickedCell)>,
    /// Time and column of the last press on a column edge.
    pub(super) last_edge_click: Option<(Instant, usize)>,
    pub(super) modifiers: keyboard::Modifiers,
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
pub(super) enum Drag {
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
        let rect = self.editor_rect(&regions).unwrap_or(Rectangle::new(
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
                            if self.line_toggle_at(&regions, position) {
                                shell.publish((self.on_event)(GridEvent::ToggleNode));
                                state.last_click = None;
                                return;
                            }
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
                    let parent = view
                        .tree(row, column)
                        .and_then(|t| t.lines.get(item))
                        .and_then(|l| match l.what {
                            What::Node { at, .. } | What::Slot(at) => Some(at.parent),
                            What::More(_) => None,
                        });
                    match parent {
                        Some(NodeKind::Struct) => Some(MenuTarget::Field),
                        Some(NodeKind::Map) => Some(MenuTarget::Entry),
                        _ => Some(MenuTarget::Item),
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
                // Space opens and closes structs, lists and maps.
                if key.as_ref() == keyboard::Key::Named(Named::Space)
                    && !ctrl
                    && view.selected_is_container()
                {
                    shell.publish((self.on_event)(GridEvent::ToggleNode));
                    shell.capture_event();
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
        self.draw_grid(tree, renderer, theme, style, layout, cursor, viewport);
    }
}

impl<'a, Message: 'a> From<Grid<'a, Message>> for Element<'a, Message> {
    fn from(grid: Grid<'a, Message>) -> Self {
        Element::new(grid)
    }
}
