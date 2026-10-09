//! View functions for the parts of the window around the grid and ribbon.

use std::path::PathBuf;

use iced::widget::{button, center, column, container, row, rule, scrollable, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use veta_core::display::{abbreviate, human_bytes};
use veta_core::{Document, DocumentId, SourceMode};

use crate::grid::MenuTarget;
use crate::icon::{Icon, icon};
use crate::theme::Tokens;
use crate::{Edit, FORMULA_ID, MenuItem, Message, Tab, Ui, bold};

const SIDE_PANE_WIDTH: f32 = 300.0;

pub fn errors(errors: &[String], ui: Ui) -> Element<'_, Message> {
    column(errors.iter().enumerate().map(|(i, e)| {
        container(
            row![
                text(e).size(ui.small()).width(Length::Fill),
                button(text("Dismiss").size(ui.small()))
                    .style(button::text)
                    .on_press(Message::DismissError(i)),
            ]
            .align_y(Alignment::Center),
        )
        .padding([6, 12])
        .width(Length::Fill)
        .style(container::danger)
        .into()
    }))
    .into()
}

pub fn empty_state(opening: bool, recent: &[PathBuf], ui: Ui) -> Element<'_, Message> {
    let t = ui.tokens;
    let content: Element<'_, Message> = if opening {
        text("Opening…").size(ui.heading()).into()
    } else {
        let mut col = column![
            icon(Icon::TableProperties, 48).color(t.muted_text),
            text("No file open").size(ui.size * 1.7),
            text("Open a Parquet file or drop one on the window.").color(t.muted_text),
            button(
                row![icon(Icon::FolderOpen, ui.size), text("Open…")]
                    .spacing(8)
                    .align_y(Alignment::Center)
            )
            .padding([6, 14])
            .on_press(Message::OpenDialog),
        ]
        .spacing(12)
        .align_x(Alignment::Center);

        if !recent.is_empty() {
            let mut list = column![text("Recent files").font(bold()).size(ui.small())].spacing(2);
            for path in recent {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let dir = path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                list = list.push(
                    button(
                        column![text(name), text(dir).size(ui.small()).color(t.muted_text),]
                            .spacing(1),
                    )
                    .width(Length::Fill)
                    .padding([4, 8])
                    .style(move |_theme: &Theme, status| subtle_button(t, status))
                    .on_press(Message::FilesPicked(vec![path.clone()])),
                );
            }
            col = col.push(container(list).width(420).padding([16, 0]));
        }
        col.into()
    };
    center(content).into()
}

/// Read-only file details. Becomes the home of applied steps, statistics and
/// metadata editing in later milestones.
pub fn side_pane(doc: &Document, ui: Ui) -> Element<'_, Message> {
    let t = ui.tokens;
    let mut fields: Vec<(&str, String)> = vec![
        ("Rows", doc.num_rows().to_string()),
        ("Columns", doc.num_columns().to_string()),
        (
            "Loaded",
            match doc.source_mode() {
                SourceMode::InMemory => "In memory".into(),
                SourceMode::Paged => "Paged from disk".into(),
            },
        ),
    ];
    if let Some(info) = doc.file_info() {
        fields.push(("File size", human_bytes(info.file_size)));
        fields.push(("Uncompressed", human_bytes(info.uncompressed_size())));
        fields.push(("Row groups", info.row_groups.len().to_string()));
    }
    fields.push(("Format", doc.writer_settings().format_version.to_string()));
    if let Some(created_by) = &doc.metadata().created_by {
        fields.push(("Created by", created_by.clone()));
    }
    let mut items = column![text("File").size(ui.heading()).font(bold())].spacing(6);
    for (label, value) in fields {
        items = items.push(
            row![
                text(label).size(ui.small()).width(110).color(t.muted_text),
                text(value).size(ui.small()),
            ]
            .spacing(8),
        );
    }

    let metadata = &doc.metadata().key_value;
    let mut meta = column![text("Metadata").size(ui.heading()).font(bold())].spacing(6);
    if metadata.is_empty() {
        meta = meta.push(text("None").size(ui.small()).color(t.muted_text));
    }
    for kv in metadata {
        meta = meta.push(
            column![
                text(kv.key.clone()).size(ui.small()).font(bold()),
                text(abbreviate(kv.value.as_deref().unwrap_or_default(), 40)).size(ui.small()),
            ]
            .spacing(2),
        );
    }

    container(scrollable(column![items, meta].spacing(20).padding(12)))
        .width(SIDE_PANE_WIDTH)
        .height(Length::Fill)
        .style(move |_theme: &Theme| container::Style::default().background(t.background))
        .into()
}

pub fn tab_bar(
    tabs: impl Iterator<Item = (DocumentId, String)>,
    active: Option<DocumentId>,
    ui: Ui,
) -> Element<'static, Message> {
    let t = ui.tokens;
    let mut bar = row![].spacing(2).align_y(Alignment::Center);
    for (id, title) in tabs {
        let is_active = active == Some(id);
        let tab = row![
            button(text(title).size(ui.small()))
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .padding([4, 8])
                .on_press(Message::SelectTab(id)),
            button(icon(Icon::Close, ui.small()))
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .padding([4, 6])
                .on_press(Message::CloseTab(id)),
        ]
        .align_y(Alignment::Center);
        bar = bar.push(container(tab).style(move |_theme: &Theme| {
            if is_active {
                container::Style {
                    background: Some(t.background.into()),
                    border: Border {
                        color: t.border,
                        width: 1.0,
                        radius: iced::border::Radius::default().bottom(4.0),
                    },
                    text_color: Some(t.primary),
                    ..container::Style::default()
                }
            } else {
                container::Style::default()
            }
        }));
    }
    bar = bar.push(
        button(icon(Icon::Plus, ui.size))
            .style(move |_theme: &Theme, status| subtle_button(t, status))
            .padding([4, 10])
            .on_press(Message::OpenDialog),
    );
    container(bar)
        .padding(iced::Padding::from([0, 6]).bottom(2))
        .width(Length::Fill)
        .style(move |_theme: &Theme| surface(t))
        .into()
}

pub fn status_bar(
    active: Option<(&Tab, &Document)>,
    opening: usize,
    ui: Ui,
) -> Element<'static, Message> {
    let mut parts: Vec<String> = Vec::new();
    if let Some((tab, doc)) = active {
        parts.push(format!("{} rows", doc.num_rows()));
        parts.push(format!("{} columns", doc.num_columns()));
        parts.push(
            match doc.source_mode() {
                SourceMode::InMemory => "In memory",
                SourceMode::Paged => "Paged",
            }
            .into(),
        );
        if let Some((r, c)) = tab.grid.selected() {
            let name = doc
                .schema()
                .fields()
                .get(c)
                .map(|f| f.name().clone())
                .unwrap_or_default();
            parts.push(format!("Row {}, {name}", r + 1));
        }
        parts.push(
            if doc.is_modified() {
                "Unsaved changes"
            } else {
                "No changes"
            }
            .into(),
        );
        if let Some(e) = tab.grid.error() {
            parts.push(format!("Read error: {e}"));
        }
    }
    if opening > 0 {
        parts.push(format!("Opening {opening} file(s)…"));
    }
    let t = ui.tokens;
    container(
        text(parts.join("   ·   "))
            .size(ui.small())
            .color(t.muted_text),
    )
    .padding([4, 12])
    .width(Length::Fill)
    .style(move |_theme: &Theme| surface(t))
    .into()
}

/// Shows and edits the selected cell, like Excel's formula bar.
pub fn formula_bar<'a>(
    tab: &'a Tab,
    doc: &'a Document,
    edit: Option<&'a Edit>,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let selected = tab.grid.selected();
    let label = match selected {
        Some((row, column)) => {
            let name = doc
                .schema()
                .fields()
                .get(column)
                .map(|f| f.name().clone())
                .unwrap_or_default();
            format!("{name} · row {}", row + 1)
        }
        None => String::new(),
    };
    let current = selected.and_then(|(r, c)| tab.grid.value(r, c));
    let (value, placeholder) = match (edit, current) {
        (Some(edit), _) => (edit.text.as_str(), ""),
        (None, Some(Some(value))) => (value, ""),
        (None, Some(None)) => ("", "null"),
        (None, None) => ("", ""),
    };
    let mut input = text_input(placeholder, value)
        .id(FORMULA_ID)
        .size(ui.small())
        .padding([3, 6]);
    if selected.is_some() {
        input = input
            .on_input(Message::EditInput)
            .on_submit(Message::CommitEdit);
    }
    let hint = if edit.is_some() {
        "Enter to apply · Esc to cancel"
    } else {
        ""
    };
    container(
        row![
            text(label)
                .size(ui.small())
                .width(200)
                .color(t.muted_text)
                .wrapping(text::Wrapping::None),
            icon(Icon::Function, ui.size).color(t.muted_text),
            input,
            text(hint).size(ui.small()).color(t.muted_text),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .width(Length::Fill)
    .style(move |_theme: &Theme| container::Style::default().background(t.background))
    .into()
}

/// Right-click menu for cells, rows or columns. `rows` is how many rows are
/// selected.
pub fn context_menu<'a>(
    target: MenuTarget,
    rows: usize,
    columns: usize,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let rows_label = |verb: &str, rest: &str| {
        if rows == 1 {
            format!("{verb} row{rest}")
        } else {
            format!("{verb} {rows} rows{rest}")
        }
    };
    let mut items: Vec<Option<(Icon, String, &str, MenuItem)>> = Vec::new();
    if target == MenuTarget::Cell {
        items.push(Some((
            Icon::Rename,
            "Edit cell".into(),
            "Enter",
            MenuItem::EditCell,
        )));
        items.push(Some((
            Icon::Eraser,
            "Set to null".into(),
            "Delete",
            MenuItem::ClearCell,
        )));
        items.push(None);
    }
    if matches!(target, MenuTarget::Cell | MenuTarget::Rows) {
        items.push(Some((
            Icon::InsertRows,
            rows_label("Insert", " above"),
            "Ctrl++",
            MenuItem::InsertRowsAbove,
        )));
        items.push(Some((
            Icon::InsertRows,
            rows_label("Insert", " below"),
            "",
            MenuItem::InsertRowsBelow,
        )));
        items.push(Some((
            Icon::Trash,
            rows_label("Delete", ""),
            "Ctrl+-",
            MenuItem::DeleteRows,
        )));
    }
    if let MenuTarget::Column(_) = target {
        let remove = if columns == 1 {
            "Remove column".to_owned()
        } else {
            format!("Remove {columns} columns")
        };
        items.extend([
            Some((
                Icon::Plus,
                "Insert column left…".into(),
                "",
                MenuItem::InsertColumnLeft,
            )),
            Some((
                Icon::Plus,
                "Insert column right…".into(),
                "",
                MenuItem::InsertColumnRight,
            )),
            Some((Icon::Rename, "Rename…".into(), "", MenuItem::RenameColumn)),
            None,
            Some((Icon::Swap, "Move left".into(), "", MenuItem::MoveColumnLeft)),
            Some((
                Icon::Swap,
                "Move right".into(),
                "",
                MenuItem::MoveColumnRight,
            )),
            None,
            Some((Icon::Trash, remove, "", MenuItem::RemoveColumns)),
        ]);
    }

    let mut list = column![].spacing(1);
    for item in items {
        list = match item {
            None => list.push(
                container(
                    rule::horizontal(1).style(move |_theme: &Theme| rule::Style {
                        color: t.border,
                        radius: 0.0.into(),
                        fill_mode: rule::FillMode::Full,
                        snap: true,
                    }),
                )
                .padding([3, 0]),
            ),
            Some((glyph, label, shortcut, message)) => list.push(
                button(
                    row![
                        icon(glyph, ui.small()).color(t.muted_text),
                        text(label).size(ui.small()).width(Length::Fill),
                        text(shortcut).size(ui.small() - 1.0).color(t.muted_text),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([5, 10])
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::Menu(message)),
            ),
        };
    }
    container(list)
        .width(240)
        .padding(4)
        .style(move |_theme: &Theme| {
            container::Style::default()
                .background(t.background)
                .color(t.text)
                .border(Border {
                    color: t.border,
                    width: 1.0,
                    radius: 6.0.into(),
                })
                .shadow(iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.2),
                    offset: iced::Vector::new(0.0, 3.0),
                    blur_radius: 12.0,
                })
        })
        .into()
}

fn surface(t: Tokens) -> container::Style {
    container::Style::default()
        .background(t.surface)
        .color(t.text)
}

fn subtle_button(t: Tokens, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Pressed => 0.2,
        button::Status::Hovered => 0.1,
        _ => 0.0,
    };
    button::Style {
        background: Some(Background::Color(Color {
            a: alpha,
            ..t.primary
        })),
        text_color: t.text,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
