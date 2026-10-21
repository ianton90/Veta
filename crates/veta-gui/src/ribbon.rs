//! The ribbon: tabs of grouped commands (Home, Rows, Columns, Nested,
//! View). Every tab is always shown; commands that don't apply to the
//! selection are greyed out.
//!
//! Every command is an [`Action`]; keyboard shortcuts emit the same actions.
//! Commands that are not built yet are shown disabled, with the issue that
//! tracks them in the tooltip.

use iced::widget::{Space, button, column, container, row, rule, text, tooltip};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use veta_core::i18n::tr;

use crate::platform::icon::{Icon, icon};
use crate::settings::config::ModePreference;
use crate::settings::theme::Tokens;

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
    DataType,
    ReplaceValues,
    FillDown,
    FillUp,
    SplitColumn,
    MergeColumns,
    FormatText,
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

    /// A longer tooltip for commands whose label alone doesn't say enough.
    fn help(self) -> Option<Help> {
        let help = |text, before, after| {
            Some(Help {
                text,
                before,
                after,
            })
        };
        match self {
            Action::ExpandStruct => help(
                "ST_RIBBON_HELP_EXPAND_STRUCT",
                "point\n{x: 1, y: 2}",
                "point.x  point.y\n1        2",
            ),
            Action::ExpandAll => help(
                "ST_RIBBON_HELP_EXPAND_ALL",
                "a\n{x: 1, b: {y: 2}}",
                "a.x  a.b.y\n1    2",
            ),
            Action::ExplodeList => help(
                "ST_RIBBON_HELP_EXPLODE_LIST",
                "id  tags\n1   [p, q]",
                "id  tags\n1   p\n1   q",
            ),
            Action::CombineStruct => help(
                "ST_RIBBON_HELP_COMBINE_STRUCT",
                "point.x  point.y\n1        2",
                "point\n{x: 1, y: 2}",
            ),
            Action::CollectList => help(
                "ST_RIBBON_HELP_COLLECT_LIST",
                "id  tags\n1   p\n1   q",
                "id  tags\n1   [p, q]",
            ),
            _ => None,
        }
    }
}

/// What a command does, with a small example of a table before and after.
struct Help {
    /// Tag of the text.
    text: &'static str,
    before: &'static str,
    after: &'static str,
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
            Tab::Home => "ST_RIBBON_TAB_HOME",
            Tab::Rows => "ST_RIBBON_TAB_ROWS",
            Tab::Columns => "ST_RIBBON_TAB_COLUMNS",
            Tab::Nested => "ST_RIBBON_TAB_NESTED",
            Tab::View => "ST_RIBBON_TAB_VIEW",
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
    /// Tag of the label (see `veta_core::i18n`).
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
    /// Tag of the label.
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
                "ST_RIBBON_GROUP_FILE",
                vec![
                    large(A::New, "ST_RIBBON_BTN_NEW", Icon::FilePlus),
                    large(A::Open, "ST_RIBBON_BTN_OPEN", Icon::FolderOpen),
                    large(A::Save, "ST_RIBBON_BTN_SAVE", Icon::Save),
                    small(A::SaveAs, "ST_RIBBON_BTN_SAVE_AS", Icon::SaveAll),
                    small(A::Close, "ST_RIBBON_BTN_CLOSE", Icon::Close),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_PROPERTIES",
                vec![
                    small(
                        A::FileMetadata,
                        "ST_RIBBON_BTN_METADATA",
                        Icon::TableProperties,
                    ),
                    small(
                        A::WriterSettings,
                        "ST_RIBBON_BTN_WRITER_SETTINGS",
                        Icon::Sliders,
                    ),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_IMPORT_EXPORT",
                vec![
                    small(A::Import, "ST_RIBBON_BTN_IMPORT", Icon::FileDown),
                    small(A::Export, "ST_RIBBON_BTN_EXPORT", Icon::FileUp),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_RECIPE",
                vec![
                    small(A::LoadRecipe, "ST_RIBBON_BTN_LOAD_RECIPE", Icon::RecipeLoad),
                    small(A::SaveRecipe, "ST_RIBBON_BTN_SAVE_RECIPE", Icon::RecipeSave),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_CLIPBOARD",
                vec![
                    large(A::Paste, "ST_RIBBON_BTN_PASTE", Icon::Paste),
                    small(A::Cut, "ST_RIBBON_BTN_CUT", Icon::Scissors),
                    small(A::Copy, "ST_RIBBON_BTN_COPY", Icon::Copy),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_EDIT",
                vec![
                    small(A::Undo, "ST_RIBBON_BTN_UNDO", Icon::Undo),
                    small(A::Redo, "ST_RIBBON_BTN_REDO", Icon::Redo),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_DATA",
                vec![
                    large(A::FilterRows, "ST_RIBBON_BTN_FILTER", Icon::Filter),
                    small(A::KeepRows, "ST_RIBBON_BTN_KEEP_ROWS", Icon::FilterPlus),
                    small(A::RemoveRows, "ST_RIBBON_BTN_REMOVE_ROWS", Icon::Eraser),
                    small(
                        A::ReplaceValues,
                        "ST_RIBBON_BTN_REPLACE_VALUES",
                        Icon::Replace,
                    ),
                    small(
                        A::SortAscending,
                        "ST_RIBBON_BTN_SORT_ASCENDING",
                        Icon::SortAsc,
                    ),
                    small(A::CustomSort, "ST_RIBBON_BTN_CUSTOM_SORT", Icon::Sliders),
                    small(
                        A::SortDescending,
                        "ST_RIBBON_BTN_SORT_DESCENDING",
                        Icon::SortDesc,
                    ),
                ],
            ),
        ],
        Tab::Rows => vec![
            group(
                "ST_RIBBON_GROUP_ROWS",
                vec![
                    large(A::FilterRows, "ST_RIBBON_BTN_FILTER", Icon::Filter),
                    large(A::KeepRows, "ST_RIBBON_BTN_KEEP_ROWS", Icon::FilterPlus),
                    large(A::RemoveRows, "ST_RIBBON_BTN_REMOVE_ROWS", Icon::Eraser),
                    large(
                        A::InsertRowsAt,
                        "ST_RIBBON_BTN_INSERT_ROWS",
                        Icon::InsertRows,
                    ),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_SORT",
                vec![
                    small(
                        A::SortAscending,
                        "ST_RIBBON_BTN_SORT_ASCENDING",
                        Icon::SortAsc,
                    ),
                    small(
                        A::SortDescending,
                        "ST_RIBBON_BTN_SORT_DESCENDING",
                        Icon::SortDesc,
                    ),
                    small(A::CustomSort, "ST_RIBBON_BTN_CUSTOM_SORT", Icon::Sliders),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_FILL",
                vec![
                    small(A::FillDown, "ST_RIBBON_BTN_FILL_DOWN", Icon::FillDown),
                    small(A::FillUp, "ST_RIBBON_BTN_FILL_UP", Icon::FillUp),
                ],
            ),
        ],
        Tab::Columns => vec![
            group(
                "ST_RIBBON_GROUP_MANAGE",
                vec![
                    small(A::RenameColumn, "ST_RIBBON_BTN_RENAME", Icon::Rename),
                    small(
                        A::MoveColumnLeft,
                        "ST_RIBBON_BTN_MOVE_LEFT",
                        Icon::ArrowLeft,
                    ),
                    small(
                        A::MoveColumnRight,
                        "ST_RIBBON_BTN_MOVE_RIGHT",
                        Icon::ArrowRight,
                    ),
                    large(A::KeepColumns, "ST_RIBBON_BTN_KEEP_COLUMNS", Icon::Columns),
                    large(
                        A::RemoveColumns,
                        "ST_RIBBON_BTN_REMOVE_COLUMNS",
                        Icon::Trash,
                    ),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_ADD",
                vec![
                    large(A::AddColumn, "ST_RIBBON_BTN_EMPTY_COLUMN", Icon::Plus),
                    large(
                        A::CustomColumn,
                        "ST_RIBBON_BTN_FORMULA_COLUMN",
                        Icon::MathSymbols,
                    ),
                    small(
                        A::IndexColumn,
                        "ST_RIBBON_BTN_INDEX_COLUMN",
                        Icon::ListOrdered,
                    ),
                    small(A::DuplicateColumn, "ST_RIBBON_BTN_DUPLICATE", Icon::Copy),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_TRANSFORM",
                vec![
                    large(A::DataType, "ST_RIBBON_BTN_DATA_TYPE", Icon::Type),
                    small(
                        A::ReplaceValues,
                        "ST_RIBBON_BTN_REPLACE_VALUES",
                        Icon::Replace,
                    ),
                    small(A::FormatText, "ST_RIBBON_BTN_FORMAT", Icon::CaseUpper),
                    small(A::SplitColumn, "ST_RIBBON_BTN_SPLIT_COLUMN", Icon::Split),
                    small(A::MergeColumns, "ST_RIBBON_BTN_MERGE_COLUMNS", Icon::Merge),
                ],
            ),
            // Empty until the function library (#74).
            group("ST_RIBBON_GROUP_CALCULATE", vec![]),
        ],
        // Each command is named by what it turns into what; "Nest" undoes
        // "Flatten" in the same order.
        Tab::Nested => vec![
            group(
                "ST_RIBBON_GROUP_FLATTEN",
                vec![
                    large(
                        A::ExpandStruct,
                        "ST_RIBBON_BTN_STRUCT_TO_COLUMNS",
                        Icon::ListTree,
                    ),
                    large(A::ExplodeList, "ST_RIBBON_BTN_LIST_TO_ROWS", Icon::Rows),
                    large(
                        A::ExpandAll,
                        "ST_RIBBON_BTN_ALL_LEVELS_TO_COLUMNS",
                        Icon::Layers,
                    ),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_NEST",
                vec![
                    large(
                        A::CombineStruct,
                        "ST_RIBBON_BTN_COLUMNS_TO_STRUCT",
                        Icon::Group,
                    ),
                    large(
                        A::CollectList,
                        "ST_RIBBON_BTN_ROWS_TO_LISTS",
                        Icon::ListCollapse,
                    ),
                ],
            ),
        ],
        Tab::View => vec![
            group(
                "ST_RIBBON_GROUP_LAYOUT",
                vec![
                    large(
                        A::ToggleDetails,
                        "ST_RIBBON_BTN_DETAILS_PANE",
                        Icon::PanelRight,
                    ),
                    small(
                        A::AppliedSteps,
                        "ST_RIBBON_BTN_APPLIED_STEPS",
                        Icon::ListTree,
                    ),
                    small(
                        A::StatisticsPane,
                        "ST_RIBBON_BTN_STATISTICS",
                        Icon::ChartColumn,
                    ),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_THEME",
                vec![
                    small(
                        A::Mode(ModePreference::Light),
                        "ST_RIBBON_BTN_LIGHT",
                        Icon::Sun,
                    ),
                    small(
                        A::Mode(ModePreference::Dark),
                        "ST_RIBBON_BTN_DARK",
                        Icon::Moon,
                    ),
                    small(
                        A::Mode(ModePreference::System),
                        "ST_RIBBON_BTN_FOLLOW_SYSTEM",
                        Icon::Monitor,
                    ),
                ],
            ),
            group(
                "ST_RIBBON_GROUP_OPTIONS",
                vec![large(A::Settings, "ST_RIBBON_BTN_SETTINGS", Icon::Settings)],
            ),
            group(
                "ST_RIBBON_GROUP_HELP",
                vec![large(A::About, "ST_RIBBON_BTN_ABOUT", Icon::Info)],
            ),
        ],
    }
}

/// What the ribbon needs to know about the app to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub has_document: bool,
    /// Rows are selected, or a cell.
    pub has_selection: bool,
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
            button(text(tr(tab.label())).size(context.text_size - 1.0))
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
            tip(tr(if ribbon.collapsed {
                "ST_RIBBON_SHOW"
            } else {
                "ST_RIBBON_HIDE"
            })),
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
        text(tr(group.label))
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
            text(tr(item.label))
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
            text(tr(item.label))
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
        Some(shortcut) => format!("{} ({shortcut})", tr(item.label)),
        None => tr(item.label),
    };
    let content: Element<'a, Message> = match item.action.help() {
        Some(help) => help_view(tip_text, help, tokens),
        None => tip(tip_text).into(),
    };
    tooltip(b, content, tooltip::Position::Bottom)
        .gap(4)
        .padding(6)
        .style(move |_theme: &Theme| tooltip_style(tokens))
        .into()
}

/// A command's title, what it does, and its example: the table before, an
/// arrow and the table after.
fn help_view<'a, Message: 'a>(title: String, help: Help, tokens: Tokens) -> Element<'a, Message> {
    let table = |rows: &'static str| {
        container(text(rows).size(12).font(iced::Font::MONOSPACE))
            .padding([4, 8])
            .style(move |_theme: &Theme| {
                container::Style::default()
                    .background(tokens.surface)
                    .border(Border {
                        color: tokens.border,
                        width: 1.0,
                        radius: 3.0.into(),
                    })
            })
    };
    column![
        tip(title).font(crate::app::bold()),
        tip(tr(help.text)).width(300).color(tokens.muted_text),
        row![
            table(help.before),
            tip("→").color(tokens.muted_text),
            table(help.after),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(6)
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
    fn nested_commands_explain_themselves() {
        for group in groups(Tab::Nested) {
            for item in group.items {
                assert!(item.action.help().is_some(), "{} has no help", item.label);
            }
        }
    }

    #[test]
    fn only_calculate_is_empty() {
        for tab in Tab::ALL {
            for group in groups(tab) {
                assert!(
                    !group.items.is_empty() || group.label == "ST_RIBBON_GROUP_CALCULATE",
                    "{} is empty",
                    group.label
                );
            }
        }
    }
}
