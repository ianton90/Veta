//! The ribbon: tabs of grouped commands (Home, Rows, Columns, Nested,
//! View). Every tab is always shown; commands that don't apply to the
//! selection are greyed out.
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
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Import,
    Export,
    LoadRecipe,
    SaveRecipe,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    KeepColumns,
    RemoveColumns,
    AddColumn,
    DuplicateColumn,
    RenameColumn,
    MoveColumnLeft,
    MoveColumnRight,
    FilterRows,
    KeepRows,
    RemoveRows,
    /// Insert a number of rows at the top, bottom or a position.
    InsertRowsAt,
    SortAscending,
    SortDescending,
    CustomSort,
    ExpandStruct,
    CombineStruct,
    CollectList,
    ExpandAll,
    ExplodeList,
    EditJson,
    DataType,
    ReplaceValues,
    FillDown,
    FillUp,
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
    fn shortcut(self) -> Option<&'static str> {
        match self {
            Action::New => Some("Ctrl+N"),
            Action::Open => Some("Ctrl+O"),
            Action::Close => Some("Ctrl+W"),
            Action::Save => Some("Ctrl+S"),
            Action::SaveAs => Some("Ctrl+Shift+S"),
            Action::Undo => Some("Ctrl+Z"),
            Action::Redo => Some("Ctrl+Y"),
            Action::Copy => Some("Ctrl+C"),
            Action::Cut => Some("Ctrl+X"),
            Action::Paste => Some("Ctrl+V"),
            Action::Settings => Some("Ctrl+,"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Home,
    Rows,
    Columns,
    Nested,
    View,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Home, Tab::Rows, Tab::Columns, Tab::Nested, Tab::View];

    fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Rows => "Rows",
            Tab::Columns => "Columns",
            Tab::Nested => "Nested",
            Tab::View => "View",
        }
    }
}

/// What kind of column is selected (or holds the selected cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnKind {
    #[default]
    None,
    /// Any non-nested column.
    Plain,
    Struct {
        /// It has struct fields itself.
        nested_structs: bool,
    },
    /// A list or map column.
    List,
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
                    large(A::New, "New", Icon::FilePlus),
                    large(A::Open, "Open", Icon::FolderOpen),
                    large(A::Save, "Save", Icon::Save),
                    small(A::SaveAs, "Save As", Icon::SaveAll),
                    small(A::Close, "Close", Icon::Close),
                ],
            ),
            group(
                "Properties",
                vec![
                    small(A::FileMetadata, "Metadata", Icon::TableProperties),
                    small(A::WriterSettings, "Writer Settings", Icon::Sliders),
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
                "Recipe",
                vec![
                    small(A::LoadRecipe, "Load Recipe", Icon::RecipeLoad),
                    small(A::SaveRecipe, "Save Recipe", Icon::RecipeSave),
                ],
            ),
            group(
                "Clipboard",
                vec![
                    large(A::Paste, "Paste", Icon::Paste),
                    small(A::Cut, "Cut", Icon::Scissors),
                    small(A::Copy, "Copy", Icon::Copy),
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
                "Data",
                vec![
                    large(A::FilterRows, "Filter", Icon::Filter),
                    small(A::SortAscending, "Sort Ascending", Icon::SortAsc),
                    small(A::SortDescending, "Sort Descending", Icon::SortDesc),
                    small(A::DataType, "Data Type", Icon::Type),
                    small(A::ReplaceValues, "Replace Values", Icon::Replace),
                    small(A::KeepColumns, "Keep Columns", Icon::Columns),
                    small(A::RemoveColumns, "Remove Columns", Icon::Trash),
                ],
            ),
        ],
        Tab::Rows => vec![
            group(
                "Rows",
                vec![
                    large(A::FilterRows, "Filter", Icon::Filter),
                    large(A::KeepRows, "Keep Rows", Icon::FilterPlus),
                    large(A::RemoveRows, "Remove Rows", Icon::Eraser),
                    large(A::InsertRowsAt, "Insert Rows", Icon::InsertRows),
                ],
            ),
            group(
                "Sort",
                vec![
                    small(A::SortAscending, "Sort Ascending", Icon::SortAsc),
                    small(A::SortDescending, "Sort Descending", Icon::SortDesc),
                    small(A::CustomSort, "Custom Sort", Icon::Sliders),
                ],
            ),
            group(
                "Fill",
                vec![
                    small(A::FillDown, "Fill Down", Icon::FillDown),
                    small(A::FillUp, "Fill Up", Icon::FillUp),
                ],
            ),
        ],
        Tab::Columns => vec![
            group(
                "Add",
                vec![
                    large(A::AddColumn, "New Column", Icon::Plus),
                    large(A::CustomColumn, "Custom Column", Icon::Function),
                    small(A::IndexColumn, "Index Column", Icon::ListOrdered),
                    small(A::DuplicateColumn, "Duplicate", Icon::Copy),
                ],
            ),
            group(
                "Manage",
                vec![
                    small(A::RenameColumn, "Rename", Icon::Rename),
                    small(A::MoveColumnLeft, "Move Left", Icon::ArrowLeft),
                    small(A::MoveColumnRight, "Move Right", Icon::ArrowRight),
                    large(A::KeepColumns, "Keep Columns", Icon::Columns),
                    large(A::RemoveColumns, "Remove Columns", Icon::Trash),
                ],
            ),
            group(
                "Transform",
                vec![
                    large(A::DataType, "Data Type", Icon::Type),
                    small(A::ReplaceValues, "Replace Values", Icon::Replace),
                    small(A::FormatText, "Format", Icon::CaseUpper),
                    small(A::SplitColumn, "Split Column", Icon::Split),
                    small(A::MergeColumns, "Merge Columns", Icon::Merge),
                ],
            ),
            group(
                "Calculate",
                vec![
                    small(A::Standard, "Standard", Icon::Calculator),
                    small(A::Statistics, "Statistics", Icon::Sigma),
                    small(A::DateTime, "Date", Icon::Calendar),
                ],
            ),
        ],
        Tab::Nested => vec![
            group(
                "Structs",
                vec![
                    large(A::ExpandStruct, "Expand", Icon::ListTree),
                    large(A::ExpandAll, "Expand All Levels", Icon::Layers),
                    large(A::CombineStruct, "Combine", Icon::Group),
                ],
            ),
            group(
                "Lists and Maps",
                vec![
                    large(A::ExplodeList, "Explode to Rows", Icon::Rows),
                    large(A::CollectList, "Collect into Lists", Icon::ListCollapse),
                ],
            ),
            group(
                "Value",
                vec![large(A::EditJson, "Edit as JSON", Icon::Braces)],
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
    /// Rows are selected, or a cell.
    pub has_selection: bool,
    /// A cell is selected (not whole rows or columns).
    pub has_cell: bool,
    /// The document has rows.
    pub has_rows: bool,
    /// The document can be changed (no filter view, step preview or
    /// computation in the way).
    pub editable: bool,
    pub column: ColumnKind,
    /// A temporary row filter is on.
    pub filter_active: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub details_visible: bool,
    pub mode: ModePreference,
    pub tokens: Tokens,
    pub text_size: f32,
}

impl Context {
    fn enabled(&self, action: Action) -> bool {
        use Action as A;
        let column = self.column != ColumnKind::None;
        match action {
            A::New | A::Open | A::Import | A::LoadRecipe => true,
            A::Undo => self.can_undo,
            A::Redo => self.can_redo,
            A::Copy => self.has_selection || column,
            A::Cut | A::Paste => self.editable && (self.has_selection || column),
            A::InsertRowsAt => self.editable,
            A::KeepRows | A::RemoveRows => self.filter_active || (self.editable && self.has_rows),
            A::RenameColumn
            | A::MoveColumnLeft
            | A::MoveColumnRight
            | A::DuplicateColumn
            | A::DataType
            | A::ReplaceValues
            | A::FillDown
            | A::FillUp
            | A::SplitColumn
            | A::MergeColumns
            | A::CombineStruct
            | A::CollectList
            | A::FormatText
            | A::Statistics
            | A::Standard
            | A::DateTime
            | A::SortAscending
            | A::SortDescending => self.editable && column,
            A::ExpandStruct => self.editable && matches!(self.column, ColumnKind::Struct { .. }),
            A::ExpandAll => {
                self.editable
                    && matches!(
                        self.column,
                        ColumnKind::Struct {
                            nested_structs: true
                        }
                    )
            }
            A::ExplodeList => self.editable && self.column == ColumnKind::List,
            A::EditJson => {
                self.editable
                    && self.has_cell
                    && matches!(self.column, ColumnKind::Struct { .. } | ColumnKind::List)
            }
            A::AddColumn
            | A::CustomColumn
            | A::IndexColumn
            | A::KeepColumns
            | A::RemoveColumns
            | A::CustomSort => self.editable,
            A::Close
            | A::Save
            | A::SaveAs
            | A::Export
            | A::SaveRecipe
            | A::FilterRows
            | A::WriterSettings
            | A::FileMetadata => self.has_document,
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

    let tip_text = match item.action.shortcut() {
        Some(shortcut) => format!("{} ({shortcut})", item.label),
        None => item.label.to_owned(),
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
    fn no_empty_groups() {
        for tab in Tab::ALL {
            for group in groups(tab) {
                assert!(!group.items.is_empty(), "{} is empty", group.label);
            }
        }
    }
}
