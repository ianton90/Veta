//! View functions for the parts of the window around the grid and ribbon.
//! Larger parts have their own file; small ones (errors, empty state, tab
//! bar, filter bar, progress) are here.

use std::path::PathBuf;

use iced::widget::{
    button, center, column, container, progress_bar, row, rule, scrollable, text, text_input,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use veta_core::display::{abbreviate, human_bytes};
use veta_core::{Document, DocumentId, SourceMode};

use crate::app::{Edit, FORMULA_ID, MenuItem, Message, SideTab, Tab, Ui, bold};
use crate::grid::MenuTarget;
use crate::platform::icon::{Icon, icon};
use crate::ribbon::Action;
use crate::settings::theme::Tokens;

use veta_core::arrow::datatypes::DataType;
use veta_core::stats::ColumnStats;
use veta_core::steps::Status;

mod formula_bar;
mod menus;
mod side_pane;
mod status_bar;

pub use formula_bar::*;
pub use menus::*;
pub use side_pane::*;
pub use status_bar::*;

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
            text("Start a new file, open a Parquet file or drop one on the window.")
                .color(t.muted_text),
            row![
                button(
                    row![icon(Icon::FilePlus, ui.size), text("New")]
                        .spacing(8)
                        .align_y(Alignment::Center)
                )
                .padding([6, 14])
                .on_press(Message::Action(crate::ribbon::Action::New)),
                button(
                    row![icon(Icon::FolderOpen, ui.size), text("Open…")]
                        .spacing(8)
                        .align_y(Alignment::Center)
                )
                .padding([6, 14])
                .on_press(Message::OpenDialog),
            ]
            .spacing(8),
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
            .on_press(Message::Action(crate::ribbon::Action::New)),
    );
    container(bar)
        .padding(iced::Padding::from([0, 6]).bottom(2))
        .width(Length::Fill)
        .style(move |_theme: &Theme| surface(t))
        .into()
}

/// The bar above the grid while a temporary filter is on.
pub fn filter_bar<'a>(tab: &Tab, doc: &Document, ui: Ui) -> Option<Element<'a, Message>> {
    let t = ui.tokens;
    let filter = tab.grid.filter()?;
    let counts = tab.grid.filter_counts(doc);
    let summary = match counts {
        Some((shown, total)) => format!(
            "{} · {} of {} rows",
            filter
                .describe()
                .replacen("Kept rows where", "Showing rows where", 1),
            abbreviate_count(shown),
            abbreviate_count(total)
        ),
        None => "Filtering…".to_owned(),
    };
    let action = |label: &'static str, message: Option<Message>| {
        let mut b = button(text(label).size(ui.small()))
            .padding([3, 8])
            .style(move |_theme: &Theme, status| subtle_button(t, status));
        if let Some(message) = message {
            b = b.on_press(message);
        }
        b
    };
    let ready = counts.is_some();
    let bar = row![
        icon(Icon::Filter, ui.small()).color(t.primary),
        text(summary).size(ui.small()).width(Length::Fill),
        action("Edit", Some(Message::Action(Action::FilterRows))),
        action(
            "Keep rows",
            ready.then_some(Message::Action(Action::KeepRows))
        ),
        action(
            "Remove rows",
            ready.then_some(Message::Action(Action::RemoveRows))
        ),
        action("Clear", Some(Message::ClearFilter)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .padding([3, 8]);
    Some(
        container(bar)
            .width(Length::Fill)
            .style(move |_theme: &Theme| {
                container::Style::default().background(Color {
                    a: 0.1,
                    ..t.primary
                })
            })
            .into(),
    )
}

fn abbreviate_count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Covers the grid while a step's full pass runs.
pub fn computing<'a>(label: String, progress: f32, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
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
    // A faint wash over the table, which stays readable.
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding(16)
        .style(move |_theme: &Theme| {
            container::Style::default().background(Color {
                a: 0.45,
                ..t.background
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
