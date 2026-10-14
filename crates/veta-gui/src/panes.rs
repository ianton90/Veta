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
use crate::ribbon::Action;
use crate::theme::Tokens;
use crate::{Edit, FORMULA_ID, MenuItem, Message, SideTab, Tab, Ui, bold};
use std::collections::HashSet;

use veta_core::arrow::datatypes::DataType;
use veta_core::display::{PathPart, ValueNode};
use veta_core::stats::ColumnStats;
use veta_core::steps::Status;
use veta_core::values::NestedEdit;

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

/// What the Value tab shows: the cell's tree, collapsed nodes and the leaf
/// being edited (pointer, text, error).
pub struct ValueView<'v> {
    pub tree: Option<ValueNode>,
    pub collapsed: &'v HashSet<String>,
    pub leaf_edit: Option<LeafEditView<'v>>,
}

/// The side pane: applied steps or file details.
pub fn side_pane<'a>(
    tab: &'a Tab,
    doc: &'a Document,
    side: SideTab,
    stats: StatsView,
    value: ValueView<'_>,
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
        tab_button("Steps", SideTab::Steps),
        tab_button("Stats", SideTab::Stats),
        tab_button("Value", SideTab::Value),
        tab_button("File", SideTab::File)
    ]
    .spacing(4);
    let body = match side {
        SideTab::Steps => steps_pane(tab, doc, ui),
        SideTab::Stats => stats_pane(stats, ui),
        SideTab::Value => value_pane(value.tree, value.collapsed, value.leaf_edit, ui),
        SideTab::File => file_details(doc, ui),
    };
    container(column![container(tabs).padding([8, 12]), body])
        .width(SIDE_PANE_WIDTH)
        .height(Length::Fill)
        .style(move |_theme: &Theme| container::Style::default().background(t.background))
        .into()
}

/// The selected cell's value as an expandable tree.
/// Expand/explode entries for a column of `column_type`.
fn nested_items(
    column_type: Option<&DataType>,
) -> Vec<Option<(Icon, String, &'static str, MenuItem)>> {
    match column_type {
        Some(DataType::Struct(fields)) => {
            let mut items = vec![Some((
                Icon::ListTree,
                "Expand fields to columns".into(),
                "",
                MenuItem::ExpandStruct,
            ))];
            if fields
                .iter()
                .any(|f| matches!(f.data_type(), DataType::Struct(_)))
            {
                items.push(Some((
                    Icon::ListTree,
                    "Expand all levels".into(),
                    "",
                    MenuItem::ExpandAll,
                )));
            }
            items
        }
        Some(
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(..)
            | DataType::Map(..),
        ) => vec![Some((
            Icon::Rows,
            "Explode to rows".into(),
            "",
            MenuItem::ExplodeList,
        ))],
        _ => Vec::new(),
    }
}

/// The value (or map key) being edited in the Value tab.
#[derive(Debug, Clone, Copy)]
pub struct LeafEditView<'v> {
    pub pointer: &'v [PathPart],
    pub text: &'v str,
    pub error: Option<&'v str>,
    /// Renaming the map key rather than editing the value.
    pub rename: bool,
}

fn value_pane<'a>(
    value: Option<ValueNode>,
    collapsed: &HashSet<String>,
    leaf_edit: Option<LeafEditView<'_>>,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let Some(root) = value else {
        return container(
            text("Select a cell to see its value.")
                .size(ui.small())
                .color(t.muted_text),
        )
        .padding([0, 12])
        .into();
    };
    let mut lines = column![
        button(text("Edit as JSON…").size(ui.small()))
            .padding([2, 8])
            .style(move |_theme: &Theme, status| subtle_button(t, status))
            .on_press(Message::OpenJsonEditor)
    ]
    .spacing(2);
    let mut stack: Vec<(ValueNode, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        let has_children = !node.children.is_empty();
        let open = !collapsed.contains(&node.path);
        let marker: Element<'a, Message> = if has_children {
            let glyph = if open { "▾" } else { "▸" };
            button(text(glyph).size(ui.small()).color(t.muted_text))
                .padding(0)
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::ToggleNode(node.path.clone()))
                .into()
        } else {
            iced::widget::Space::new().width(ui.small()).into()
        };
        let value_color = if node.null || has_children {
            t.muted_text
        } else {
            t.text
        };
        // Values inside nested cells can be edited in place.
        let editing = leaf_edit.filter(|e| e.pointer == node.pointer.as_slice());
        let editor = |e: LeafEditView<'_>, placeholder: &str| -> Element<'a, Message> {
            let mut column = column![
                text_input(placeholder, e.text)
                    .id(crate::LEAF_ID)
                    .size(ui.small())
                    .padding([1, 4])
                    .on_input(Message::LeafInput)
                    .on_submit(Message::CommitLeaf)
                    .width(140)
            ];
            if let Some(error) = e.error {
                column = column.push(
                    text(error.to_owned())
                        .size(ui.small() - 1.0)
                        .color(Color::from_rgb8(0xd1, 0x43, 0x43)),
                );
            }
            column.into()
        };
        let value: Element<'a, Message> = match editing {
            Some(e) if !e.rename => editor(e, "null"),
            Some(_) => text(node.text.clone())
                .size(ui.small())
                .color(value_color)
                .into(),
            None if !has_children && !node.pointer.is_empty() => {
                let initial = if node.null {
                    String::new()
                } else {
                    node.text.clone()
                };
                button(text(node.text.clone()).size(ui.small()).color(value_color))
                    .padding([0, 3])
                    .style(move |_theme: &Theme, status| subtle_button(t, status))
                    .on_press(Message::EditLeaf(
                        node.pointer.clone(),
                        node.data_type.clone(),
                        initial,
                    ))
                    .into()
            }
            None => text(node.text.clone())
                .size(ui.small())
                .color(value_color)
                .into(),
        };
        // Map keys can be renamed; list items and map entries removed;
        // lists and maps take new items.
        let key = match node.pointer.last() {
            Some(PathPart::Key(key)) => Some(key.clone()),
            _ => None,
        };
        let label: Element<'a, Message> = match (editing, &key) {
            (Some(e), Some(_)) if e.rename => editor(e, "key"),
            (_, Some(key)) => button(text(node.label.clone()).size(ui.small()).font(bold()))
                .padding(0)
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::EditKey(node.pointer.clone(), key.clone()))
                .into(),
            _ => text(node.label.clone())
                .size(ui.small())
                .font(bold())
                .into(),
        };
        let small_button = |glyph: &'static str, message: Message| {
            button(text(glyph).size(ui.small()).color(t.muted_text))
                .padding([0, 4])
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(message)
        };
        let add = match node.data_type {
            DataType::List(_) | DataType::LargeList(_) => Some(small_button(
                "+",
                Message::NestedEdit(node.pointer.clone(), NestedEdit::AppendItem),
            )),
            DataType::Map(..) => Some(small_button(
                "+",
                Message::NestedEdit(node.pointer.clone(), NestedEdit::AddEntry),
            )),
            _ => None,
        };
        let removable = matches!(
            node.pointer.last(),
            Some(PathPart::Index(_) | PathPart::Key(_))
        );
        let remove = removable.then(|| {
            small_button(
                "✕",
                Message::NestedEdit(node.pointer.clone(), NestedEdit::Remove),
            )
        });
        lines = lines.push(
            row![
                iced::widget::Space::new().width(depth as f32 * 14.0),
                marker,
                label,
                value,
                // Containers say what they hold; leaves show their type.
                text(if has_children {
                    String::new()
                } else {
                    node.type_name.clone()
                })
                .size(ui.small() - 1.0)
                .color(Color {
                    a: 0.6,
                    ..t.muted_text
                }),
            ]
            .extend(add.map(Into::into))
            .extend(remove.map(Into::into))
            .spacing(6)
            .align_y(Alignment::Center),
        );
        if has_children && open {
            for child in node.children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    scrollable(container(lines).padding([0, 12]))
        .height(Length::Fill)
        .into()
}

/// What the statistics tab shows.
#[derive(Debug, Default)]
pub struct StatsView {
    /// Per selected column: its statistics so far, and whether they are only
    /// the quick ones from the file's footer (or none yet).
    pub columns: Vec<(String, Option<ColumnStats>, bool)>,
    /// Progress of the pass computing the missing ones.
    pub progress: Option<f32>,
    /// Why statistics can't be computed right now.
    pub waiting: Option<&'static str>,
}

/// Statistics of the selected columns.
fn stats_pane<'a>(view: StatsView, ui: Ui) -> Element<'a, Message> {
    use veta_core::stats::{format_distinct, format_float};
    let t = ui.tokens;
    let mut content = column![].spacing(10).padding([0, 12]);
    if let Some(reason) = view.waiting {
        content = content.push(text(reason).size(ui.small()).color(t.muted_text));
    }
    if view.columns.is_empty() {
        content = content.push(
            text("Select one or more columns to see their statistics.")
                .size(ui.small())
                .color(t.muted_text),
        );
    }
    if let Some(progress) = view.progress {
        content = content.push(
            column![
                text(format!("Computing… {:.0}%", progress * 100.0))
                    .size(ui.small())
                    .color(t.muted_text),
                progress_bar(0.0..=1.0, progress).girth(4),
            ]
            .spacing(4),
        );
    }
    let pending = "…".to_owned();
    for (name, stats, partial) in view.columns {
        let mut card = column![
            row![
                text(name).size(ui.size).font(bold()).width(Length::Fill),
                text(
                    stats
                        .as_ref()
                        .map(|s| s.type_name.clone())
                        .unwrap_or_default()
                )
                .size(ui.small())
                .color(t.muted_text),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(3);
        let line = |label: &'static str, value: Option<String>| {
            row![
                text(label).size(ui.small()).color(t.muted_text).width(90),
                text(value.unwrap_or_else(|| "—".into())).size(ui.small()),
            ]
        };
        let (count, nulls, distinct, min, max, mean, sd) = match &stats {
            Some(s) => {
                let percent = if s.count > 0 {
                    format!(" ({:.1}%)", s.nulls as f64 * 100.0 / s.count as f64)
                } else {
                    String::new()
                };
                let later = |v: Option<String>| if partial { Some(pending.clone()) } else { v };
                (
                    Some(s.count.to_string()),
                    Some(format!("{}{percent}", s.nulls)),
                    later(format_distinct(s)),
                    s.min.clone(),
                    s.max.clone(),
                    later(s.mean.map(format_float)),
                    later(s.std_dev.map(format_float)),
                )
            }
            None => {
                let p = Some(pending.clone());
                (
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p,
                )
            }
        };
        card = card
            .push(line("Count", count))
            .push(line("Nulls", nulls))
            .push(line("Distinct", distinct))
            .push(line("Min", min))
            .push(line("Max", max))
            .push(line("Mean", mean))
            .push(line("Std dev", sd));
        content = content.push(container(card).padding(8).width(Length::Fill).style(
            move |_theme: &Theme| {
                container::Style::default()
                    .background(t.surface)
                    .border(Border {
                        color: t.border,
                        width: 1.0,
                        radius: 6.0.into(),
                    })
            },
        ));
    }
    scrollable(content).height(Length::Fill).into()
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
    let footer_button = |label: &'static str, message: Option<Message>| {
        let mut b = button(text(label).size(ui.small()))
            .padding([4, 8])
            .style(move |_theme: &Theme, status| subtle_button(t, status));
        if let Some(message) = message {
            b = b.on_press(message);
        }
        b
    };
    let footer = row![
        footer_button(
            "Save recipe…",
            (!steps.is_empty()).then_some(Message::SaveRecipe)
        ),
        footer_button("Load recipe…", Some(Message::LoadRecipe)),
    ]
    .spacing(4)
    .padding([4, 8]);
    column![scrollable(content).height(Length::Fill), footer].into()
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
            .on_press(Message::Action(crate::ribbon::Action::New)),
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
    transfers: &[crate::Transfer],
    notice: Option<&str>,
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
                .map(|f| format!(", {}", f.name()))
                .unwrap_or_default();
            parts.push(format!("Row {}{name}", r + 1));
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
    if transfers.is_empty()
        && let Some(notice) = notice
    {
        parts.push(notice.to_owned());
    }
    let t = ui.tokens;
    let mut bar = row![
        text(parts.join("   ·   "))
            .size(ui.small())
            .color(t.muted_text)
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    for transfer in transfers {
        bar = bar.push(
            text(format!(
                "{} {:.0}%",
                transfer.label,
                transfer.progress * 100.0
            ))
            .size(ui.small())
            .color(t.text),
        );
        bar = bar.push(
            button(text("Cancel").size(ui.small()))
                .padding([0, 8])
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::CancelTransfer(transfer.id)),
        );
    }
    container(bar)
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
                .map(|f| {
                    let path = tab.grid.selected_field().map_or(&[][..], |(p, _)| p);
                    let mut name = f.name().clone();
                    for part in path {
                        name = format!("{name}.{part}");
                    }
                    if let Some((item, _, _)) = tab.grid.selected_item() {
                        name = format!("{name}[{item}]");
                    }
                    format!("{name} · ")
                })
                .unwrap_or_default();
            format!("{name}row {}", row + 1)
        }
        None => String::new(),
    };
    let current = tab.grid.selected_edit_value();
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

/// One entry of a right-click menu: icon, label, shortcut, action.
type MenuEntry = (Icon, String, &'static str, MenuItem);

/// The right-click menu's sections for `target`: a title and its entries.
pub fn menu_sections(
    target: MenuTarget,
    rows: usize,
    columns: usize,
    column_type: Option<&DataType>,
) -> Vec<(&'static str, Vec<MenuEntry>)> {
    let entry = |icon, label: &str, shortcut, item| (icon, label.to_owned(), shortcut, item);
    let count = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_owned()
        } else {
            many.replace("{n}", &n.to_string())
        }
    };
    let nested = column_type.is_some_and(DataType::is_nested);
    let mut sections: Vec<(&'static str, Vec<MenuEntry>)> = Vec::new();
    if target == MenuTarget::Blank {
        sections.push((
            "Add",
            vec![
                entry(Icon::InsertRows, "Add row", "Ctrl++", MenuItem::AddRow),
                entry(Icon::Plus, "Add column…", "", MenuItem::AddColumn),
            ],
        ));
        return sections;
    }
    if target == MenuTarget::Item {
        sections.push((
            "List item",
            vec![
                entry(Icon::Rename, "Edit item", "Enter", MenuItem::EditCell),
                entry(
                    Icon::InsertRows,
                    "Insert item above",
                    "Ctrl++",
                    MenuItem::InsertItem,
                ),
                entry(
                    Icon::InsertRows,
                    "Insert item below",
                    "",
                    MenuItem::InsertItemBelow,
                ),
                entry(Icon::Trash, "Delete item", "Ctrl+-", MenuItem::DeleteItem),
                entry(Icon::Eraser, "Set to null", "Delete", MenuItem::ClearCell),
            ],
        ));
        return sections;
    }
    sections.push((
        "Clipboard",
        vec![
            entry(Icon::Scissors, "Cut", "Ctrl+X", MenuItem::Cut),
            entry(Icon::Copy, "Copy", "Ctrl+C", MenuItem::Copy),
            entry(Icon::Paste, "Paste", "Ctrl+V", MenuItem::Paste),
        ],
    ));
    if target == MenuTarget::Cell {
        let mut cell = vec![
            entry(Icon::Rename, "Edit cell", "Enter", MenuItem::EditCell),
            entry(Icon::Eraser, "Set to null", "Delete", MenuItem::ClearCell),
        ];
        if nested {
            cell.push(entry(Icon::Braces, "Edit as JSON…", "", MenuItem::EditJson));
        }
        sections.push(("Cell", cell));
        sections.push((
            "Filter",
            vec![
                entry(
                    Icon::Filter,
                    "Filter by this value",
                    "",
                    MenuItem::KeepValue,
                ),
                entry(
                    Icon::Filter,
                    "Filter out this value",
                    "",
                    MenuItem::ExcludeValue,
                ),
            ],
        ));
    }
    if matches!(target, MenuTarget::Cell | MenuTarget::Rows) {
        sections.push((
            "Rows",
            vec![
                (
                    Icon::InsertRows,
                    count(rows, "Insert row above", "Insert {n} rows above"),
                    "Ctrl++",
                    MenuItem::InsertRowsAbove,
                ),
                (
                    Icon::InsertRows,
                    count(rows, "Insert row below", "Insert {n} rows below"),
                    "",
                    MenuItem::InsertRowsBelow,
                ),
                (
                    Icon::FilterPlus,
                    count(rows, "Keep only this row", "Keep only these {n} rows"),
                    "",
                    MenuItem::KeepRows,
                ),
                (
                    Icon::Trash,
                    count(rows, "Delete row", "Delete {n} rows"),
                    "Ctrl+-",
                    MenuItem::DeleteRows,
                ),
            ],
        ));
    }
    if let MenuTarget::Column(_) = target {
        sections.push((
            "Column",
            vec![
                entry(
                    Icon::Plus,
                    "Insert column left…",
                    "",
                    MenuItem::InsertColumnLeft,
                ),
                entry(
                    Icon::Plus,
                    "Insert column right…",
                    "",
                    MenuItem::InsertColumnRight,
                ),
                entry(Icon::Rename, "Rename…", "", MenuItem::RenameColumn),
                entry(Icon::Copy, "Duplicate", "", MenuItem::DuplicateColumn),
                entry(Icon::ArrowLeft, "Move left", "", MenuItem::MoveColumnLeft),
                entry(
                    Icon::ArrowRight,
                    "Move right",
                    "",
                    MenuItem::MoveColumnRight,
                ),
                (
                    Icon::Columns,
                    count(
                        columns,
                        "Keep only this column",
                        "Keep only these {n} columns",
                    ),
                    "",
                    MenuItem::KeepColumns,
                ),
                (
                    Icon::Trash,
                    count(columns, "Remove column", "Remove {n} columns"),
                    "",
                    MenuItem::RemoveColumns,
                ),
            ],
        ));
        sections.push((
            "Data",
            vec![
                entry(Icon::Filter, "Filter…", "", MenuItem::FilterColumn),
                entry(Icon::SortAsc, "Sort ascending", "", MenuItem::SortAscending),
                entry(
                    Icon::SortDesc,
                    "Sort descending",
                    "",
                    MenuItem::SortDescending,
                ),
                entry(Icon::Type, "Change type…", "", MenuItem::ChangeType),
                entry(
                    Icon::Replace,
                    "Replace values…",
                    "",
                    MenuItem::ReplaceValues,
                ),
                entry(Icon::CaseUpper, "Format text…", "", MenuItem::FormatText),
                entry(Icon::FillDown, "Fill down", "", MenuItem::FillDown),
                entry(Icon::FillUp, "Fill up", "", MenuItem::FillUp),
            ],
        ));
        sections.push((
            "Split & Merge",
            vec![
                entry(Icon::Split, "Split column…", "", MenuItem::SplitColumn),
                entry(Icon::Merge, "Merge columns…", "", MenuItem::MergeColumns),
            ],
        ));
    }
    let mut nested_entries: Vec<MenuEntry> =
        nested_items(column_type).into_iter().flatten().collect();
    if matches!(target, MenuTarget::Column(_)) {
        nested_entries.push(entry(
            Icon::Group,
            "Combine into struct…",
            "",
            MenuItem::CombineStruct,
        ));
        nested_entries.push(entry(
            Icon::ListCollapse,
            "Collect into lists…",
            "",
            MenuItem::CollectList,
        ));
    }
    if !nested_entries.is_empty() {
        sections.push(("Nested", nested_entries));
    }
    sections
}

/// Width of the right-click menu.
pub const MENU_WIDTH: f32 = 250.0;

/// Approximate height of a menu with these sections, to keep it on screen.
pub fn menu_height(sections: &[(&'static str, Vec<MenuEntry>)], ui: Ui) -> f32 {
    let entries: usize = sections.iter().map(|(_, e)| e.len()).sum();
    let item = ui.small() * 1.3 + 10.0 + 1.0;
    let title = ui.small() * 1.3 + 8.0;
    entries as f32 * item + sections.len() as f32 * (title + 7.0) + 10.0
}

pub fn context_menu<'a>(
    sections: Vec<(&'static str, Vec<MenuEntry>)>,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let mut list = column![].spacing(1);
    let count = sections.len();
    for (i, (title, entries)) in sections.into_iter().enumerate() {
        list = list.push(
            container(
                text(title)
                    .size(ui.small() - 1.0)
                    .font(bold())
                    .color(t.muted_text),
            )
            .padding(iced::Padding::from([2, 10]).top(4)),
        );
        for (glyph, label, shortcut, message) in entries {
            list = list.push(
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
            );
        }
        if i + 1 < count {
            list = list.push(
                container(
                    rule::horizontal(1).style(move |_theme: &Theme| rule::Style {
                        color: t.border,
                        radius: 0.0.into(),
                        fill_mode: rule::FillMode::Full,
                        snap: true,
                    }),
                )
                .padding([3, 0]),
            );
        }
    }
    container(list)
        .width(MENU_WIDTH)
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
