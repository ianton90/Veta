//! View functions for the parts of the window around the grid.

use iced::widget::{button, center, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Font, Length, Theme, font};
use veta_core::display::{abbreviate, human_bytes};
use veta_core::{Document, DocumentId, SourceMode};

use crate::{Message, Tab};

const SIDE_PANE_WIDTH: f32 = 300.0;

fn bold() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }
}

/// Placeholder for the ribbon (#31): ribbon tab names and the actions that
/// exist so far.
pub fn ribbon(has_document: bool, side_pane: bool) -> Element<'static, Message> {
    let tabs = row(["Home", "Transform", "Add Column", "View"].map(|name| {
        let label = text(name).size(13);
        if name == "Home" {
            label.font(bold()).into()
        } else {
            label.style(text::secondary).into()
        }
    }))
    .spacing(18);

    let actions = row![
        button(text("Open…").size(13)).on_press(Message::OpenDialog),
        button(text("Close").size(13))
            .style(button::secondary)
            .on_press_maybe(has_document.then_some(Message::CloseActiveTab)),
        space::horizontal(),
        button(
            text(if side_pane {
                "Hide details"
            } else {
                "Show details"
            })
            .size(13)
        )
        .style(button::secondary)
        .on_press(Message::ToggleSidePane),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(column![tabs, actions].spacing(8))
        .padding([8, 12])
        .width(Length::Fill)
        .style(bar_style)
        .into()
}

/// Background for the ribbon and the tab bar.
fn bar_style(theme: &Theme) -> container::Style {
    container::Style::default().background(theme.extended_palette().background.weak.color)
}

pub fn errors(errors: &[String]) -> Element<'_, Message> {
    column(errors.iter().enumerate().map(|(i, e)| {
        container(
            row![
                text(e).size(13).width(Length::Fill),
                button(text("Dismiss").size(12))
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

pub fn empty_state(opening: bool) -> Element<'static, Message> {
    let content: Element<'_, Message> = if opening {
        text("Opening…").size(18).into()
    } else {
        column![
            text("No file open").size(24),
            text("Open a Parquet file or drop one on the window.").style(text::secondary),
            button(text("Open…")).on_press(Message::OpenDialog),
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .into()
    };
    center(content).into()
}

/// Read-only file details. Becomes the home of applied steps, statistics and
/// metadata editing in later milestones.
pub fn side_pane(doc: &Document) -> Element<'_, Message> {
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
    let mut items = column![text("File").size(15).font(bold())].spacing(6);
    for (label, value) in fields {
        items = items.push(
            row![
                text(label).size(13).width(110).style(text::secondary),
                text(value).size(13),
            ]
            .spacing(8),
        );
    }

    let metadata = &doc.metadata().key_value;
    let mut meta = column![text("Metadata").size(15).font(bold())].spacing(6);
    if metadata.is_empty() {
        meta = meta.push(text("None").size(13).style(text::secondary));
    }
    for kv in metadata {
        meta = meta.push(
            column![
                text(kv.key.clone()).size(13).font(bold()),
                text(abbreviate(kv.value.as_deref().unwrap_or_default(), 40)).size(13),
            ]
            .spacing(2),
        );
    }

    container(scrollable(column![items, meta].spacing(20).padding(12)))
        .width(SIDE_PANE_WIDTH)
        .height(Length::Fill)
        .into()
}

pub fn tab_bar(
    tabs: impl Iterator<Item = (DocumentId, String)>,
    active: Option<DocumentId>,
) -> Element<'static, Message> {
    let mut bar = row![].spacing(2).align_y(Alignment::Center);
    for (id, title) in tabs {
        let is_active = active == Some(id);
        let tab = row![
            button(text(title).size(13))
                .style(button::text)
                .padding([4, 8])
                .on_press(Message::SelectTab(id)),
            button(text("×").size(13))
                .style(button::text)
                .padding([4, 6])
                .on_press(Message::CloseTab(id)),
        ]
        .align_y(Alignment::Center);
        bar = bar.push(container(tab).style(move |theme: &Theme| {
            if is_active {
                container::Style {
                    background: Some(theme.extended_palette().background.base.color.into()),
                    border: iced::Border {
                        color: theme.extended_palette().primary.base.color,
                        width: 1.0,
                        radius: 2.0.into(),
                    },
                    ..container::Style::default()
                }
            } else {
                container::Style::default()
            }
        }));
    }
    bar = bar.push(
        button(text("+").size(14))
            .style(button::text)
            .padding([4, 10])
            .on_press(Message::OpenDialog),
    );
    container(bar)
        .padding([2, 6])
        .width(Length::Fill)
        .style(bar_style)
        .into()
}

pub fn status_bar(active: Option<(&Tab, &Document)>, opening: usize) -> Element<'static, Message> {
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
        parts.push("No changes".into());
        if let Some(e) = tab.grid.error() {
            parts.push(format!("Read error: {e}"));
        }
    }
    if opening > 0 {
        parts.push(format!("Opening {opening} file(s)…"));
    }
    container(text(parts.join("   ·   ")).size(12))
        .padding([4, 12])
        .width(Length::Fill)
        .into()
}
