//! View functions for the parts of the window around the grid and ribbon.

use std::path::PathBuf;

use iced::widget::{
    button, center, column, container, progress_bar, row, rule, scrollable, text, text_input,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use veta_core::display::{abbreviate, human_bytes};
use veta_core::{Document, DocumentId, SourceMode};

use crate::grid::MenuTarget;
use crate::icon::{Icon, icon};
use crate::theme::Tokens;
use crate::{Edit, FORMULA_ID, MenuItem, Message, SideTab, Tab, Ui, bold};
use veta_core::steps::Status;

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

/// The side pane: applied steps or file details.
pub fn side_pane<'a>(
    tab: &'a Tab,
    doc: &'a Document,
    side: SideTab,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let tab_button = |label: &'static str, target: SideTab| {
        let selected = side == target;
        button(text(label).size(ui.small()))
            .padding([4, 10])
            .style(move |_theme: &Theme, status| {
                let mut style = subtle_button(t, status);
                if selected {
                    style.text_color = t.primary;
                    style.border = Border {
                        color: t.primary,
                        width: 0.0,
                        radius: 4.0.into(),
                    };
                    style.background = Some(Background::Color(Color {
                        a: 0.1,
                        ..t.primary
                    }));
                }
                style
            })
            .on_press(Message::SideTab(target))
    };
    let tabs = row![
        tab_button("Applied steps", SideTab::Steps),
        tab_button("File", SideTab::File)
    ]
    .spacing(4);
    let body = match side {
        SideTab::Steps => steps_pane(tab, doc, ui),
        SideTab::File => file_details(doc, ui),
    };
    container(column![container(tabs).padding([8, 12]), body])
        .width(SIDE_PANE_WIDTH)
        .height(Length::Fill)
        .style(move |_theme: &Theme| container::Style::default().background(t.background))
        .into()
}

/// The applied steps list. Selecting a step shows the data after it.
fn steps_pane<'a>(tab: &'a Tab, doc: &'a Document, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
    let steps = doc.steps();
    let evaluated = doc.evaluated_steps();
    let shown = tab.grid.preview().unwrap_or(evaluated).min(evaluated);
    let red = Color::from_rgb8(0xd1, 0x43, 0x43);

    let item = |index: Option<usize>, label: String, note: Option<(String, Color)>| {
        // `index` is the step position; None is the source.
        let level = index.map_or(0, |i| i + 1);
        let selected = level == shown;
        let available = level <= evaluated;
        let color = if available { t.text } else { t.muted_text };
        let mut text_col = column![text(label).size(ui.small()).color(color)].spacing(2);
        if let Some((note, note_color)) = note {
            text_col = text_col.push(text(note).size(ui.small() - 1.0).color(note_color));
        }
        let mut select = button(text_col.width(Length::Fill))
            .width(Length::Fill)
            .padding([5, 8])
            .style(move |_theme: &Theme, status| {
                let mut style = subtle_button(t, status);
                if selected {
                    style.background = Some(Background::Color(Color {
                        a: 0.14,
                        ..t.selection
                    }));
                }
                style
            });
        if available {
            select = select.on_press(Message::PreviewStep(tab.id, level));
        }
        let mut line = row![select].spacing(2).align_y(Alignment::Center);
        if let Some(i) = index {
            let small = |glyph: Icon, message: Option<Message>, tip: &'static str| {
                let color = if message.is_some() {
                    t.muted_text
                } else {
                    Color { a: 0.2, ..t.text }
                };
                let mut b = button(icon(glyph, ui.small()).color(color))
                    .padding([4, 5])
                    .style(move |_theme: &Theme, status| subtle_button(t, status));
                if let Some(message) = message {
                    b = b.on_press(message);
                }
                iced::widget::tooltip(
                    b,
                    text(tip).size(ui.small() - 1.0),
                    iced::widget::tooltip::Position::Top,
                )
            };
            let editable = crate::step_dialog_supported(&steps[i]);
            line = line
                .push(small(
                    Icon::ChevronUp,
                    i.checked_sub(1).map(|up| Message::MoveStep(i, up)),
                    "Move up",
                ))
                .push(small(
                    Icon::ChevronDown,
                    (i + 1 < steps.len()).then_some(Message::MoveStep(i, i + 1)),
                    "Move down",
                ))
                .push(small(
                    Icon::Rename,
                    editable.then_some(Message::EditStep(i)),
                    "Edit",
                ))
                .push(small(Icon::Trash, Some(Message::RemoveStep(i)), "Remove"));
        }
        line.into()
    };

    let mut list: Vec<Element<'a, Message>> = vec![item(None, "Source".into(), None)];
    for (i, step) in steps.iter().enumerate() {
        let note = match doc.status() {
            Status::Broken { step, error } if *step == i => Some((error.clone(), red)),
            Status::Broken { step, .. } if i > *step => {
                Some(("Not evaluated".into(), t.muted_text))
            }
            Status::Computing { step } if *step == i => Some(("Computing…".into(), t.primary)),
            Status::Computing { step } if i > *step => Some(("Waiting".into(), t.muted_text)),
            _ => None,
        };
        list.push(item(
            Some(i),
            format!("{}. {}", i + 1, step.describe()),
            note,
        ));
    }
    let mut content = column(list).spacing(2).padding([0, 8]);
    if steps.is_empty() {
        content = content.push(
            text("Changes you make appear here as steps.")
                .size(ui.small())
                .color(t.muted_text),
        );
    }
    scrollable(content).height(Length::Fill).into()
}

/// Read-only file details and metadata.
fn file_details(doc: &Document, ui: Ui) -> Element<'_, Message> {
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

    // The Arrow schema entry is internal to the file format.
    let metadata: Vec<_> = doc
        .metadata()
        .key_value
        .iter()
        .filter(|kv| kv.key != veta_core::io::ARROW_SCHEMA_KEY)
        .collect();
    let mut meta = column![
        row![
            text("Metadata").size(ui.heading()).font(bold()),
            iced::widget::Space::new().width(Length::Fill),
            button(text("Edit…").size(ui.small()))
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::Action(crate::ribbon::Action::FileMetadata)),
        ]
        .align_y(Alignment::Center)
    ]
    .spacing(6);
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

    scrollable(column![items, meta].spacing(20).padding(12))
        .height(Length::Fill)
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
        match doc.status() {
            Status::Broken { step, .. } => parts.push(format!("Step {} has an error", step + 1)),
            Status::Computing { step } => parts.push(format!("Computing step {}…", step + 1)),
            Status::Ready => {}
        }
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
    if let Some(at) = tab.grid.preview() {
        let total = doc.steps().len();
        let what = if at == 0 {
            "the source data".to_owned()
        } else {
            format!("the data after step {at} of {total}")
        };
        return container(
            row![
                icon(Icon::Info, ui.size).color(t.primary),
                text(format!("Showing {what}. Changes are disabled.")).size(ui.small()),
                button(text("Show all steps").size(ui.small()))
                    .padding([3, 10])
                    .on_press(Message::PreviewStep(tab.id, usize::MAX)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .width(Length::Fill)
        .style(move |_theme: &Theme| {
            container::Style::default().background(Color {
                a: 0.1,
                ..t.primary
            })
        })
        .into();
    }
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
    let (hint, hint_color) = match edit {
        Some(Edit {
            error: Some(error), ..
        }) => (error.as_str(), Color::from_rgb8(0xd1, 0x43, 0x43)),
        Some(_) => ("Enter to apply · Esc to cancel", t.muted_text),
        None => ("", t.muted_text),
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
            text(hint).size(ui.small()).color(hint_color),
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
/// Covers the grid while a step's full pass runs.
pub fn computing<'a>(doc: &Document, step: usize, progress: f32, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
    let label = doc
        .steps()
        .get(step)
        .map_or_else(String::new, |s| format!("{}. {}", step + 1, s.describe()));
    let card = container(
        column![
            text("Computing…").size(ui.heading()).font(bold()),
            text(label).size(ui.small()).color(t.muted_text),
            progress_bar(0.0..=1.0, progress).girth(8),
            row![
                text(format!("{:.0}%", progress * 100.0)).size(ui.small()),
                iced::widget::Space::new().width(Length::Fill),
                button(text("Cancel").size(ui.small()))
                    .style(button::secondary)
                    .on_press(Message::CancelCompute),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(10)
        .width(360),
    )
    .padding(20)
    .style(move |_theme: &Theme| {
        container::Style::default()
            .background(t.background)
            .color(t.text)
            .border(Border {
                color: t.border,
                width: 1.0,
                radius: 8.0.into(),
            })
    });
    center(card)
        .style(|_theme: &Theme| {
            container::Style::default().background(Color::from_rgba(0.0, 0.0, 0.0, 0.2))
        })
        .into()
}

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
        items.push(Some((
            Icon::Filter,
            "Keep rows with this value".into(),
            "",
            MenuItem::KeepValue,
        )));
        items.push(Some((
            Icon::Filter,
            "Remove rows with this value".into(),
            "",
            MenuItem::ExcludeValue,
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
            Some((Icon::Filter, "Filter…".into(), "", MenuItem::FilterColumn)),
            Some((
                Icon::SortAsc,
                "Sort ascending".into(),
                "",
                MenuItem::SortAscending,
            )),
            Some((
                Icon::SortDesc,
                "Sort descending".into(),
                "",
                MenuItem::SortDescending,
            )),
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
