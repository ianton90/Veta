//! The ribbon: tabs of grouped commands, modelled on Power Query's.
//!
//! Every command is an [`Action`]; keyboard shortcuts emit the same actions.
//! Commands that are not built yet are shown disabled, with the issue that
//! tracks them in the tooltip.

use iced::widget::{Space, button, column, container, row, rule, text, tooltip};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::config::ModePreference;
use crate::icon::{Icon, icon};
use crate::theme::Tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Save,
    SaveAs,
    Close,
    Import,
    Export,
    Undo,
    Redo,
    ChooseColumns,
    RemoveColumns,
    AddColumn,
    DuplicateColumn,
    RenameColumn,
    KeepRows,
    RemoveRows,
    InsertRows,
    SortAscending,
    SortDescending,
    DataType,
    ReplaceValues,
    FillDown,
    SplitColumn,
    MergeColumns,
    FormatText,
    Statistics,
    Standard,
    DateTime,
    CustomColumn,
    IndexColumn,
    ToggleDetails,
    AppliedSteps,
    StatisticsPane,
    FileMetadata,
    WriterSettings,
    Mode(ModePreference),
    Settings,
    About,
}

impl Action {
    /// Issue tracking a command that is not built yet.
    pub fn planned(self) -> Option<u32> {
        Some(match self {
            Action::Import => 45,
            Action::Export => 46,
            Action::DuplicateColumn => 40,
            Action::DataType => 35,
            Action::ReplaceValues | Action::FillDown | Action::FormatText => 38,
            Action::SplitColumn | Action::MergeColumns => 39,
            Action::Statistics => 42,
            Action::Standard | Action::DateTime | Action::CustomColumn | Action::IndexColumn => 40,
            Action::StatisticsPane => 43,
            Action::Open
            | Action::AppliedSteps
            | Action::FileMetadata
            | Action::WriterSettings
            | Action::Save
            | Action::SaveAs
            | Action::AddColumn
            | Action::ChooseColumns
            | Action::RemoveColumns
            | Action::RenameColumn
            | Action::InsertRows
            | Action::RemoveRows
            | Action::KeepRows
            | Action::SortAscending
            | Action::SortDescending
            | Action::Undo
            | Action::Redo
            | Action::Close
            | Action::ToggleDetails
            | Action::Mode(_)
            | Action::Settings
            | Action::About => return None,
        })
    }

    fn shortcut(self) -> Option<&'static str> {
        match self {
            Action::Open => Some("Ctrl+O"),
            Action::Close => Some("Ctrl+W"),
            Action::Save => Some("Ctrl+S"),
            Action::SaveAs => Some("Ctrl+Shift+S"),
            Action::Undo => Some("Ctrl+Z"),
            Action::InsertRows => Some("Ctrl++"),
            Action::RemoveRows => Some("Ctrl+-"),
            Action::Redo => Some("Ctrl+Y"),
            Action::Settings => Some("Ctrl+,"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Home,
    Transform,
    AddColumn,
    View,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Home, Tab::Transform, Tab::AddColumn, Tab::View];

    fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Transform => "Transform",
            Tab::AddColumn => "Add Column",
            Tab::View => "View",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ribbon {
    pub tab: Tab,
    pub collapsed: bool,
}

#[derive(Debug, Clone)]
pub enum RibbonMessage {
    SelectTab(Tab),
    ToggleCollapsed,
}

impl Ribbon {
    pub fn update(&mut self, message: RibbonMessage) {
        match message {
            RibbonMessage::SelectTab(tab) => {
                self.tab = tab;
                self.collapsed = false;
            }
            RibbonMessage::ToggleCollapsed => self.collapsed = !self.collapsed,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Size {
    Large,
    Small,
}

#[derive(Debug, Clone, Copy)]
struct Item {
    action: Action,
    label: &'static str,
    icon: Icon,
    size: Size,
}

const fn large(action: Action, label: &'static str, icon: Icon) -> Item {
    Item {
        action,
        label,
        icon,
        size: Size::Large,
    }
}

const fn small(action: Action, label: &'static str, icon: Icon) -> Item {
    Item {
        action,
        label,
        icon,
        size: Size::Small,
    }
}

struct Group {
    label: &'static str,
    items: Vec<Item>,
}

fn group(label: &'static str, items: Vec<Item>) -> Group {
    Group { label, items }
}

fn groups(tab: Tab) -> Vec<Group> {
    use Action as A;
    match tab {
        Tab::Home => vec![
            group(
                "File",
                vec![
                    large(A::Open, "Open", Icon::FolderOpen),
                    large(A::Save, "Save", Icon::Save),
                    small(A::SaveAs, "Save As", Icon::SaveAll),
                    small(A::Close, "Close", Icon::Close),
                ],
            ),
            group(
                "Import / Export",
                vec![
                    small(A::Import, "Import", Icon::FileDown),
                    small(A::Export, "Export", Icon::FileUp),
                ],
            ),
            group(
                "Edit",
                vec![
                    small(A::Undo, "Undo", Icon::Undo),
                    small(A::Redo, "Redo", Icon::Redo),
                ],
            ),
            group(
                "Manage Columns",
                vec![
                    large(A::ChooseColumns, "Choose Columns", Icon::Columns),
                    large(A::RemoveColumns, "Remove Columns", Icon::Trash),
                ],
            ),
            group(
                "Reduce Rows",
                vec![
                    large(A::KeepRows, "Keep Rows", Icon::Filter),
                    large(A::RemoveRows, "Remove Rows", Icon::Eraser),
                ],
            ),
            group(
                "Sort",
                vec![
                    small(A::SortAscending, "Sort Ascending", Icon::SortAsc),
                    small(A::SortDescending, "Sort Descending", Icon::SortDesc),
                ],
            ),
            group(
                "Transform",
                vec![
                    small(A::DataType, "Data Type", Icon::Type),
                    small(A::ReplaceValues, "Replace Values", Icon::Replace),
                    small(A::SplitColumn, "Split Column", Icon::Split),
                ],
            ),
        ],
        Tab::Transform => vec![
            group(
                "Any Column",
                vec![
                    large(A::DataType, "Data Type", Icon::Type),
                    small(A::RenameColumn, "Rename", Icon::Rename),
                    small(A::ReplaceValues, "Replace Values", Icon::Replace),
                    small(A::FillDown, "Fill Down", Icon::FillDown),
                ],
            ),
            group(
                "Text Column",
                vec![
                    large(A::SplitColumn, "Split Column", Icon::Split),
                    small(A::MergeColumns, "Merge Columns", Icon::Merge),
                    small(A::FormatText, "Format", Icon::CaseUpper),
                ],
            ),
            group(
                "Number Column",
                vec![
                    large(A::Statistics, "Statistics", Icon::Sigma),
                    small(A::Standard, "Standard", Icon::Calculator),
                ],
            ),
            group(
                "Date & Time",
                vec![large(A::DateTime, "Date", Icon::Calendar)],
            ),
            group(
                "Rows",
                vec![
                    small(A::InsertRows, "Insert Rows", Icon::InsertRows),
                    small(A::RemoveRows, "Remove Rows", Icon::Eraser),
                ],
            ),
        ],
        Tab::AddColumn => vec![
            group(
                "General",
                vec![
                    large(A::AddColumn, "New Column", Icon::Plus),
                    large(A::CustomColumn, "Custom Column", Icon::Function),
                    small(A::IndexColumn, "Index Column", Icon::ListOrdered),
                    small(A::DuplicateColumn, "Duplicate Column", Icon::Copy),
                ],
            ),
            group(
                "From Text",
                vec![
                    small(A::MergeColumns, "Merge Columns", Icon::Merge),
                    small(A::FormatText, "Format", Icon::CaseUpper),
                ],
            ),
            group(
                "From Number",
                vec![
                    small(A::Standard, "Standard", Icon::Calculator),
                    small(A::Statistics, "Statistics", Icon::Sigma),
                ],
            ),
        ],
        Tab::View => vec![
            group(
                "Layout",
                vec![
                    large(A::ToggleDetails, "Details Pane", Icon::PanelRight),
                    small(A::AppliedSteps, "Applied Steps", Icon::ListTree),
                    small(A::StatisticsPane, "Statistics", Icon::ChartColumn),
                ],
            ),
            group(
                "File",
                vec![
                    small(A::FileMetadata, "Metadata", Icon::TableProperties),
                    small(A::WriterSettings, "Writer Settings", Icon::Sliders),
                ],
            ),
            group(
                "Theme",
                vec![
                    small(A::Mode(ModePreference::Light), "Light", Icon::Sun),
                    small(A::Mode(ModePreference::Dark), "Dark", Icon::Moon),
                    small(
                        A::Mode(ModePreference::System),
                        "Follow System",
                        Icon::Monitor,
                    ),
                ],
            ),
            group(
                "Options",
                vec![large(A::Settings, "Settings", Icon::Settings)],
            ),
            group("Help", vec![large(A::About, "About", Icon::Info)]),
        ],
    }
}

/// What the ribbon needs to know about the app to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub has_document: bool,
    pub has_selection: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub details_visible: bool,
    pub mode: ModePreference,
    pub tokens: Tokens,
    pub text_size: f32,
}

impl Context {
    fn enabled(&self, action: Action) -> bool {
        if action.planned().is_some() {
            return false;
        }
        match action {
            Action::Close => self.has_document,
            Action::Undo => self.can_undo,
            Action::InsertRows
            | Action::RemoveRows
            | Action::RemoveColumns
            | Action::RenameColumn => self.has_selection,
            Action::Save
            | Action::SaveAs
            | Action::AddColumn
            | Action::ChooseColumns
            | Action::WriterSettings => self.has_document,
            Action::Redo => self.can_redo,
            _ => true,
        }
    }

    fn checked(&self, action: Action) -> bool {
        match action {
            Action::ToggleDetails => self.details_visible,
            Action::Mode(mode) => self.mode == mode,
            _ => false,
        }
    }
}

const SMALL_PER_COLUMN: usize = 3;

pub fn view<'a, Message: Clone + 'a>(
    ribbon: &Ribbon,
    context: Context,
    on_message: impl Fn(RibbonMessage) -> Message + Copy + 'a,
    on_action: impl Fn(Action) -> Message + Copy + 'a,
) -> Element<'a, Message> {
    let tokens = context.tokens;

    let mut strip = row![].spacing(2).align_y(Alignment::End);
    for tab in Tab::ALL {
        let selected = ribbon.tab == tab && !ribbon.collapsed;
        strip = strip.push(
            button(text(tab.label()).size(context.text_size - 1.0))
                .padding([5, 14])
                .style(move |_theme: &Theme, status| tab_style(tokens, selected, status))
                .on_press(on_message(RibbonMessage::SelectTab(tab))),
        );
    }
    let collapse_icon = if ribbon.collapsed {
        Icon::ChevronDown
    } else {
        Icon::ChevronUp
    };
    strip = strip.push(Space::new().width(Length::Fill)).push(
        tooltip(
            button(icon(collapse_icon, context.text_size))
                .padding([5, 8])
                .style(move |_theme: &Theme, status| item_style(tokens, false, status))
                .on_press(on_message(RibbonMessage::ToggleCollapsed)),
            tip(if ribbon.collapsed {
                "Show the ribbon"
            } else {
                "Hide the ribbon"
            }),
            tooltip::Position::Left,
        )
        .padding(6)
        .style(move |_theme: &Theme| tooltip_style(tokens)),
    );

    let mut content = column![container(strip).padding(iced::Padding::from([0, 8]).top(4))];

    if !ribbon.collapsed {
        let mut body = row![].spacing(0).height(context.text_size * 6.6);
        let all = groups(ribbon.tab);
        let count = all.len();
        for (i, g) in all.into_iter().enumerate() {
            body = body.push(group_view(g, context, on_action));
            if i + 1 < count {
                body = body.push(
                    container(rule::vertical(1).style(move |_theme: &Theme| rule_style(tokens)))
                        .padding([8, 0]),
                );
            }
        }
        content = content.push(container(body).width(Length::Fill).padding([4, 8]).style(
            move |_theme: &Theme| {
                container::Style::default()
                    .background(tokens.background)
                    .border(Border {
                        color: tokens.border,
                        width: 0.0,
                        radius: 0.0.into(),
                    })
            },
        ));
    }

    container(content)
        .width(Length::Fill)
        .style(move |_theme: &Theme| container::Style::default().background(tokens.surface))
        .into()
}

fn group_view<'a, Message: Clone + 'a>(
    group: Group,
    context: Context,
    on_action: impl Fn(Action) -> Message + Copy + 'a,
) -> Element<'a, Message> {
    let tokens = context.tokens;
    let mut items = row![].spacing(2).height(Length::Fill);
    let mut smalls: Vec<Item> = Vec::new();

    let flush = |items: iced::widget::Row<'a, Message>, smalls: &mut Vec<Item>| {
        if smalls.is_empty() {
            return items;
        }
        let col = column(
            smalls
                .drain(..)
                .map(|item| item_button(item, context, on_action)),
        )
        .spacing(1);
        items.push(col)
    };

    for item in group.items {
        match item.size {
            Size::Large => {
                items = flush(items, &mut smalls);
                items = items.push(item_button(item, context, on_action));
            }
            Size::Small => {
                smalls.push(item);
                if smalls.len() == SMALL_PER_COLUMN {
                    items = flush(items, &mut smalls);
                }
            }
        }
    }
    items = flush(items, &mut smalls);

    column![
        items,
        text(group.label)
            .size(context.text_size - 3.0)
            .color(tokens.muted_text)
            .wrapping(text::Wrapping::None),
    ]
    .padding([0, 6])
    .spacing(2)
    .align_x(Alignment::Center)
    .into()
}

fn item_button<'a, Message: Clone + 'a>(
    item: Item,
    context: Context,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    let tokens = context.tokens;
    let enabled = context.enabled(item.action);
    let checked = context.checked(item.action);
    let color = if enabled {
        tokens.text
    } else {
        Color {
            a: 0.38,
            ..tokens.text
        }
    };

    let size = context.text_size;
    let content: Element<'a, Message> = match item.size {
        Size::Large => column![
            icon(item.icon, (size * 1.85).round()).color(color),
            text(item.label)
                .size(size - 2.0)
                .color(color)
                .align_x(Alignment::Center)
                .wrapping(text::Wrapping::Word),
        ]
        .spacing(5)
        .width(Length::Shrink)
        .align_x(Alignment::Center)
        .into(),
        Size::Small => row![
            icon(item.icon, size).color(color),
            text(item.label)
                .size(size - 2.0)
                .color(color)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    };

    let padding = match item.size {
        Size::Large => [6, 8],
        Size::Small => [3, 6],
    };
    let mut b = button(content)
        .padding(padding)
        .style(move |_theme: &Theme, status| item_style(tokens, checked, status));
    if let Size::Large = item.size {
        b = b.height(Length::Fill).width(Length::Shrink);
    }
    if enabled {
        b = b.on_press(on_action(item.action));
    }

    let tip_text = match (item.action.planned(), item.action.shortcut()) {
        (Some(issue), _) => format!("{} — coming soon (#{issue})", item.label),
        (None, Some(shortcut)) => format!("{} ({shortcut})", item.label),
        (None, None) => item.label.to_owned(),
    };
    tooltip(b, tip(tip_text), tooltip::Position::Bottom)
        .gap(4)
        .padding(6)
        .style(move |_theme: &Theme| tooltip_style(tokens))
        .into()
}

fn tip<'a>(content: impl iced::widget::text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(content).size(12)
}

fn tooltip_style(tokens: Tokens) -> container::Style {
    container::Style::default()
        .background(tokens.background)
        .color(tokens.text)
        .border(Border {
            color: tokens.border,
            width: 1.0,
            radius: 4.0.into(),
        })
}

fn rule_style(tokens: Tokens) -> rule::Style {
    rule::Style {
        color: tokens.border,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

fn tab_style(tokens: Tokens, selected: bool, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if selected {
            tokens.background
        } else if hovered {
            Color {
                a: 0.08,
                ..tokens.text
            }
        } else {
            Color::TRANSPARENT
        })),
        text_color: if selected {
            tokens.primary
        } else {
            tokens.text
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: iced::border::Radius::default().top(4.0),
        },
        ..button::Style::default()
    }
}

fn item_style(tokens: Tokens, checked: bool, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Pressed => 0.22,
        button::Status::Hovered => 0.12,
        _ if checked => 0.16,
        _ => 0.0,
    };
    button::Style {
        background: Some(Background::Color(Color {
            a: alpha,
            ..tokens.primary
        })),
        text_color: tokens.text,
        border: Border {
            color: if checked {
                Color {
                    a: 0.5,
                    ..tokens.primary
                }
            } else {
                Color::TRANSPARENT
            },
            width: if checked { 1.0 } else { 0.0 },
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_a_tab_expands_a_collapsed_ribbon() {
        let mut ribbon = Ribbon::default();
        ribbon.update(RibbonMessage::ToggleCollapsed);
        assert!(ribbon.collapsed);
        ribbon.update(RibbonMessage::SelectTab(Tab::View));
        assert!(!ribbon.collapsed);
        assert_eq!(ribbon.tab, Tab::View);
    }

    #[test]
    fn every_planned_action_points_at_an_issue() {
        for tab in Tab::ALL {
            for group in groups(tab) {
                assert!(!group.items.is_empty(), "{} is empty", group.label);
                for item in group.items {
                    if let Some(issue) = item.action.planned() {
                        assert!((21..=52).contains(&issue), "{:?}", item.action);
                    }
                }
            }
        }
    }
}
