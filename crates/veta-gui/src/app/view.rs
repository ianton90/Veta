//! Drawing the window.

use super::*;
use iced::widget::column;

/// What the computing overlay says is running.
pub(crate) fn computing_label(doc: &Document, c: &Computing) -> String {
    match (c.target, doc.steps().get(c.step)) {
        (Target::Document, Some(step)) => format!("{}. {}", c.step + 1, step.describe()),
        _ => "Filtering rows".to_owned(),
    }
}

/// How to refer to a column in a formula.
pub(crate) fn formula_reference(column: &str) -> String {
    format!("[{column}]")
}

pub(crate) fn divider<'a>(tokens: Tokens, vertical: bool) -> Element<'a, Message> {
    let style = move |_theme: &Theme| rule::Style {
        color: tokens.border,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    };
    if vertical {
        rule::vertical(1).style(style).into()
    } else {
        rule::horizontal(1).style(style).into()
    }
}

impl App {
    pub(crate) fn view(&self) -> Element<'_, Message> {
        let ui = self.ui();
        let main: Element<'_, Message> = match self.active_document() {
            Some((tab, doc)) => {
                let id = tab.id;
                let edit = self.edit.as_ref().filter(|e| e.document == id);
                let editor = edit.filter(|e| {
                    let selected = tab
                        .grid
                        .selected()
                        .map(|(r, c)| (r, doc.schema().fields().get(c).map(|f| f.name().clone())));
                    selected == Some((e.row, Some(e.column.clone())))
                });
                let editor = editor.map(|e| {
                    iced::widget::text_input("", &e.text)
                        .id(EDITOR_ID)
                        .on_input(Message::EditInput)
                        .on_submit(Message::CommitEdit)
                        .size(ui.size - 1.0)
                        .padding([1, 5])
                        .into()
                });
                let grid =
                    grid::grid(&tab.grid, ui.tokens, move |e| Message::Grid(id, e)).editor(editor);
                let grid: Element<'_, Message> =
                    match self.computing.as_ref().filter(|c| c.document == id) {
                        // Not opaque: the table can still be scrolled and
                        // read; edits are refused until the step is done.
                        Some(c) => stack![
                            grid,
                            panes::computing(computing_label(doc, c), c.progress, ui)
                        ]
                        .into(),
                        None => grid.into(),
                    };
                let mut grid_column = column![
                    panes::formula_bar(tab, doc, edit, ui),
                    divider(ui.tokens, false),
                ];
                if let Some(bar) = panes::filter_bar(tab, doc, ui) {
                    grid_column = grid_column.push(bar);
                }
                let grid =
                    grid_column.push(container(grid).width(Length::Fill).height(Length::Fill));
                if self.show_side_pane {
                    row![
                        grid,
                        divider(ui.tokens, true),
                        panes::side_pane(
                            tab,
                            doc,
                            self.side_tab,
                            self.stats_view(id),
                            panes::ValueView {
                                tree: self.value_tree(tab, doc),
                                collapsed: &self.collapsed,
                                leaf_edit: self
                                    .leaf_edit
                                    .as_ref()
                                    .filter(|e| e.document == id)
                                    .map(|e| panes::LeafEditView {
                                        pointer: e.pointer.as_slice(),
                                        text: e.text.as_str(),
                                        error: e.error.as_deref(),
                                        rename: e.rename,
                                    }),
                            },
                            ui
                        )
                    ]
                    .into()
                } else {
                    grid.into()
                }
            }
            None => panes::empty_state(!self.opening.is_empty(), &self.config.recent_files, ui),
        };

        let column = self.active_document().and_then(|(tab, doc)| {
            let c = tab.grid.selected_columns()?.start;
            doc.schema().fields().get(c).map(|f| f.data_type().clone())
        });
        let context = ribbon::Context {
            has_document: self.active.is_some(),
            has_selection: self.active.and_then(|id| self.selected_rows(id)).is_some(),
            has_cell: self
                .active_document()
                .is_some_and(|(tab, _)| matches!(tab.grid.selection(), Selection::Cell(..))),
            editable: self.active.is_some_and(|id| self.editable(id)),
            has_rows: self
                .active_document()
                .is_some_and(|(_, doc)| doc.num_rows() > 0),
            column: match column {
                None => ribbon::ColumnKind::None,
                Some(veta_core::arrow::datatypes::DataType::Struct(fields)) => {
                    ribbon::ColumnKind::Struct {
                        nested_structs: fields.iter().any(|f| {
                            matches!(
                                f.data_type(),
                                veta_core::arrow::datatypes::DataType::Struct(_)
                            )
                        }),
                    }
                }
                Some(
                    veta_core::arrow::datatypes::DataType::List(_)
                    | veta_core::arrow::datatypes::DataType::LargeList(_)
                    | veta_core::arrow::datatypes::DataType::FixedSizeList(..)
                    | veta_core::arrow::datatypes::DataType::Map(..),
                ) => ribbon::ColumnKind::List,
                Some(_) => ribbon::ColumnKind::Plain,
            },
            filter_active: self
                .active_document()
                .is_some_and(|(tab, doc)| tab.grid.filter_counts(doc).is_some()),
            can_undo: self
                .active_document()
                .is_some_and(|(_, d)| d.history().can_undo()),
            can_redo: self
                .active_document()
                .is_some_and(|(_, d)| d.history().can_redo()),
            details_visible: self.show_side_pane,
            mode: self.config.appearance.mode,
            tokens: ui.tokens,
            text_size: ui.size,
        };
        let window = column![
            ribbon::view(&self.ribbon, context, Message::Ribbon, Message::Action),
            divider(ui.tokens, false),
            panes::errors(&self.errors, ui),
            container(main).height(Length::Fill),
            divider(ui.tokens, false),
            panes::tab_bar(
                self.tabs.iter().filter_map(|t| {
                    let doc = self.workbook.get(t.id)?;
                    let title = if doc.is_modified() {
                        format!("● {}", doc.title())
                    } else {
                        doc.title()
                    };
                    Some((t.id, title))
                }),
                self.active,
                ui,
            ),
            panes::status_bar(
                self.active_document(),
                self.opening
                    .iter()
                    .filter(|p| !self
                        .transfers
                        .iter()
                        .any(|t| t.importing.as_ref() == Some(p)))
                    .count(),
                &self.transfers,
                self.notice.as_deref(),
                ui
            ),
        ];

        if self.saving.is_some() {
            let t = ui.tokens;
            return stack![
                window,
                opaque(
                    center(
                        container(iced::widget::text("Saving…").size(ui.heading()))
                            .padding(24)
                            .style(move |_theme: &Theme| {
                                container::Style::default()
                                    .background(t.background)
                                    .color(t.text)
                                    .border(iced::Border {
                                        color: t.border,
                                        width: 1.0,
                                        radius: 8.0.into(),
                                    })
                            })
                    )
                    .style(|_theme: &Theme| {
                        container::Style::default()
                            .background(Color::from_rgba(0.0, 0.0, 0.0, 0.25))
                    })
                ),
            ]
            .into();
        }
        if let Some(menu) = &self.menu {
            let rows = self.selected_rows(menu.document).map_or(1, |r| r.len());
            let columns = self.selected_columns(menu.document).map_or(1, |c| c.len());
            let column = match menu.target {
                MenuTarget::Column(c) => Some(c),
                MenuTarget::Cell => self
                    .tabs
                    .iter()
                    .find(|t| t.id == menu.document)
                    .and_then(|t| t.grid.selected())
                    .map(|(_, c)| c),
                _ => None,
            };
            let column_type = column.and_then(|c| {
                self.workbook
                    .get(menu.document)
                    .and_then(|d| d.schema().fields().get(c).map(|f| f.data_type().clone()))
            });
            let sections = panes::menu_sections(menu.target, rows, columns, column_type.as_ref());
            // Opens up or to the left where it wouldn't fit below or right.
            let height = panes::menu_height(&sections, ui);
            let mut position = menu.position;
            if position.y + height > self.window_size.height {
                position.y = (self.window_size.height - height).max(0.0);
            }
            if position.x + panes::MENU_WIDTH > self.window_size.width {
                position.x = (position.x - panes::MENU_WIDTH).max(0.0);
            }
            return stack![
                window,
                mouse_area(
                    container(iced::widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                )
                .on_press(Message::CloseMenu)
                .on_right_press(Message::CloseMenu),
                pin(panes::context_menu(sections, ui)).position(position),
            ]
            .into();
        }
        let Some(dialog) = &self.dialog else {
            return window.into();
        };
        let card = match dialog {
            Dialog::Settings(draft) => {
                settings::view(draft, &self.themes, self.themes_dir.as_deref(), ui.tokens)
                    .map(Message::Settings)
            }
            Dialog::About => settings::about(Message::CloseDialog, ui.tokens),
            Dialog::Column { dialog, .. } => dialog.view(ui.tokens).map(Message::ColumnDialog),
            Dialog::Filter { dialog, .. } => dialog.view(ui.tokens).map(Message::Filter),
            Dialog::Sort { dialog, .. } => dialog.view(ui.tokens).map(Message::Sort),
            Dialog::Import(dialog) => dialog.view(ui.tokens).map(Message::Import),
            Dialog::Export { dialog, .. } => dialog.view(ui.tokens).map(Message::Export),
            Dialog::Cast { dialog, .. } => dialog.view(ui.tokens).map(Message::Cast),
            Dialog::Replace { dialog, .. } => dialog.view(ui.tokens).map(Message::Replace),
            Dialog::Format { dialog, .. } => dialog.view(ui.tokens).map(Message::Format),
            Dialog::Split { dialog, .. } => dialog.view(ui.tokens).map(Message::Split),
            Dialog::Custom { dialog, .. } => dialog.view(ui.tokens).map(Message::Custom),
            Dialog::Merge { dialog, .. } => dialog.view(ui.tokens).map(Message::Merge),
            Dialog::Combine { dialog, .. } => dialog.view(ui.tokens).map(Message::Combine),
            Dialog::Collect { dialog, .. } => dialog.view(ui.tokens).map(Message::Collect),
            Dialog::Rows { dialog, .. } => dialog.view(ui.tokens).map(Message::Rows),
            Dialog::ConfirmStep { .. } => dialogs::confirm_step(ui.tokens),
            Dialog::ConfirmRecipe { document, .. } => {
                let steps = self.workbook.get(*document).map_or(0, |d| d.steps().len());
                dialogs::confirm_recipe(steps, ui.tokens)
            }
            Dialog::Choose { dialog, .. } => dialog.view(ui.tokens).map(Message::Choose),
            Dialog::Writer { dialog, .. } => dialog.view(ui.tokens).map(Message::Writer),
            Dialog::Metadata { dialog, .. } => dialog.view(ui.tokens).map(Message::Metadata),
            Dialog::Json { dialog, .. } => dialog.view(ui.tokens).map(Message::Json),
            Dialog::ConfirmClose { document } => {
                let name = self
                    .workbook
                    .get(*document)
                    .map(Document::title)
                    .unwrap_or_default();
                dialogs::confirm_close(name, ui.tokens)
            }
        };
        stack![
            window,
            opaque(
                mouse_area(center(opaque(card)).style(|_theme: &Theme| {
                    container::Style::default().background(Color::from_rgba(0.0, 0.0, 0.0, 0.35))
                }))
                .on_press(Message::CloseDialog)
            )
        ]
        .into()
    }
}
