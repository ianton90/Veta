//! The right-click menu.

use super::*;
use crate::i18n::{tr, tr_with};
use iced::widget::column;
use veta_core::values::node::NodeKind;

/// One entry of a right-click menu: icon, label, shortcut, action. The
/// shortcut is shown through `tr`: key names (`ST_KEY_ENTER`) are tags,
/// key combinations ("Ctrl+X") show as they are.
type MenuEntry = (Icon, String, &'static str, MenuItem);

/// A menu entry before translation: icon, label tag, shortcut, action.
type Spec = (Icon, &'static str, &'static str, MenuItem);

/// A menu section: its title tag and entries.
type Section = (&'static str, Vec<MenuEntry>);

const BLANK: &[Spec] = &[
    (
        Icon::InsertRows,
        "ST_MENU_ADD_ROW",
        "Ctrl++",
        MenuItem::AddRow,
    ),
    (Icon::Plus, "ST_MENU_ADD_COLUMN", "", MenuItem::AddColumn),
];

const FIELD: &[Spec] = &[
    (
        Icon::Rename,
        "ST_MENU_EDIT_FIELD",
        "ST_KEY_ENTER",
        MenuItem::EditCell,
    ),
    (
        Icon::Eraser,
        "ST_MENU_SET_TO_NULL",
        "ST_KEY_DELETE",
        MenuItem::ClearCell,
    ),
    (Icon::Plus, "ST_MENU_ADD_FIELD", "", MenuItem::AddField),
    (
        Icon::Trash,
        "ST_MENU_REMOVE_FIELD",
        "",
        MenuItem::RemoveField,
    ),
];

const ENTRY: &[Spec] = &[
    (
        Icon::Rename,
        "ST_MENU_EDIT_VALUE",
        "ST_KEY_ENTER",
        MenuItem::EditCell,
    ),
    (Icon::Rename, "ST_MENU_EDIT_KEY", "", MenuItem::EditKey),
    (
        Icon::InsertRows,
        "ST_MENU_INSERT_ENTRY_ABOVE",
        "Ctrl++",
        MenuItem::InsertItem,
    ),
    (
        Icon::InsertRows,
        "ST_MENU_INSERT_ENTRY_BELOW",
        "",
        MenuItem::InsertItemBelow,
    ),
    (
        Icon::Trash,
        "ST_MENU_DELETE_ENTRY",
        "Ctrl+-",
        MenuItem::DeleteItem,
    ),
    (
        Icon::Eraser,
        "ST_MENU_SET_VALUE_TO_NULL",
        "ST_KEY_DELETE",
        MenuItem::ClearCell,
    ),
];

const ITEM: &[Spec] = &[
    (
        Icon::Rename,
        "ST_MENU_EDIT_ITEM",
        "ST_KEY_ENTER",
        MenuItem::EditCell,
    ),
    (
        Icon::InsertRows,
        "ST_MENU_INSERT_ITEM_ABOVE",
        "Ctrl++",
        MenuItem::InsertItem,
    ),
    (
        Icon::InsertRows,
        "ST_MENU_INSERT_ITEM_BELOW",
        "",
        MenuItem::InsertItemBelow,
    ),
    (
        Icon::Trash,
        "ST_MENU_DELETE_ITEM",
        "Ctrl+-",
        MenuItem::DeleteItem,
    ),
    (
        Icon::Eraser,
        "ST_MENU_SET_TO_NULL",
        "ST_KEY_DELETE",
        MenuItem::ClearCell,
    ),
];

const CELL: &[Spec] = &[
    (
        Icon::Rename,
        "ST_MENU_EDIT_CELL",
        "ST_KEY_ENTER",
        MenuItem::EditCell,
    ),
    (
        Icon::Eraser,
        "ST_MENU_SET_TO_NULL",
        "ST_KEY_DELETE",
        MenuItem::ClearCell,
    ),
];

const FILTER: &[Spec] = &[
    (
        Icon::Filter,
        "ST_MENU_FILTER_BY_THIS_VALUE",
        "",
        MenuItem::KeepValue,
    ),
    (
        Icon::Filter,
        "ST_MENU_FILTER_OUT_THIS_VALUE",
        "",
        MenuItem::ExcludeValue,
    ),
];

const COLUMN: &[Spec] = &[
    (
        Icon::Plus,
        "ST_MENU_INSERT_COLUMN_LEFT",
        "",
        MenuItem::InsertColumnLeft,
    ),
    (
        Icon::Plus,
        "ST_MENU_INSERT_COLUMN_RIGHT",
        "",
        MenuItem::InsertColumnRight,
    ),
    (Icon::Rename, "ST_MENU_RENAME", "", MenuItem::RenameColumn),
    (
        Icon::Copy,
        "ST_MENU_DUPLICATE",
        "",
        MenuItem::DuplicateColumn,
    ),
    (
        Icon::ArrowLeft,
        "ST_MENU_MOVE_LEFT",
        "",
        MenuItem::MoveColumnLeft,
    ),
    (
        Icon::ArrowRight,
        "ST_MENU_MOVE_RIGHT",
        "",
        MenuItem::MoveColumnRight,
    ),
];

const DATA: &[Spec] = &[
    (Icon::Filter, "ST_MENU_FILTER", "", MenuItem::FilterColumn),
    (
        Icon::SortAsc,
        "ST_MENU_SORT_ASCENDING",
        "",
        MenuItem::SortAscending,
    ),
    (
        Icon::SortDesc,
        "ST_MENU_SORT_DESCENDING",
        "",
        MenuItem::SortDescending,
    ),
    (Icon::Type, "ST_MENU_CHANGE_TYPE", "", MenuItem::ChangeType),
    (
        Icon::Replace,
        "ST_MENU_REPLACE_VALUES",
        "",
        MenuItem::ReplaceValues,
    ),
    (
        Icon::CaseUpper,
        "ST_MENU_FORMAT_TEXT",
        "",
        MenuItem::FormatText,
    ),
    (Icon::FillDown, "ST_MENU_FILL_DOWN", "", MenuItem::FillDown),
    (Icon::FillUp, "ST_MENU_FILL_UP", "", MenuItem::FillUp),
];

/// The right-click menu's sections for `target`: a title and its entries.
/// `line` is the kind of the selected line of an open cell, if one is.
pub fn menu_sections(
    target: MenuTarget,
    rows: usize,
    columns: usize,
    column_type: Option<&DataType>,
    line: Option<NodeKind>,
) -> Vec<Section> {
    let in_line = |title, specs| {
        let mut sections = vec![(title, entries(specs))];
        sections.extend(inside(line));
        sections
    };
    match target {
        MenuTarget::Blank => vec![("ST_MENU_SECTION_ADD", entries(BLANK))],
        MenuTarget::Field => in_line("ST_MENU_SECTION_STRUCT_FIELD", FIELD),
        MenuTarget::Entry => in_line("ST_MENU_SECTION_MAP_ENTRY", ENTRY),
        MenuTarget::Item => in_line("ST_MENU_SECTION_LIST_ITEM", ITEM),
        MenuTarget::Cell => {
            let mut cell = entries(CELL);
            cell.extend(add_inside(column_type));
            vec![
                clipboard(),
                ("ST_MENU_SECTION_CELL", cell),
                ("ST_MENU_SECTION_FILTER", entries(FILTER)),
                rows_section(rows),
            ]
        }
        MenuTarget::Rows => vec![clipboard(), rows_section(rows)],
        MenuTarget::Column(_) => {
            let mut column = entries(COLUMN);
            column.push((
                Icon::Columns,
                count(
                    columns,
                    "ST_MENU_KEEP_ONLY_THIS_COLUMN",
                    "ST_MENU_KEEP_ONLY_THESE_N_COLUMNS",
                ),
                "",
                MenuItem::KeepColumns,
            ));
            column.push((
                Icon::Trash,
                count(columns, "ST_MENU_REMOVE_COLUMN", "ST_MENU_REMOVE_N_COLUMNS"),
                "",
                MenuItem::RemoveColumns,
            ));
            column.extend(add_inside(column_type));
            vec![
                clipboard(),
                ("ST_MENU_SECTION_COLUMN", column),
                ("ST_MENU_SECTION_DATA", entries(DATA)),
            ]
        }
    }
}

/// Labels and shortcut key names are tags.
fn entries(specs: &[Spec]) -> Vec<MenuEntry> {
    specs
        .iter()
        .map(|&(icon, label, shortcut, item)| (icon, tr(label), shortcut, item))
        .collect()
}

/// `one` for one, else `many` with the number.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        tr(one)
    } else {
        tr_with(many, &[("n", &n)])
    }
}

fn clipboard() -> Section {
    let entry = |icon, label, action, item| (icon, tr(label), shortcut(action), item);
    let entries = vec![
        entry(Icon::Scissors, "ST_MENU_CUT", Action::Cut, MenuItem::Cut),
        entry(Icon::Copy, "ST_MENU_COPY", Action::Copy, MenuItem::Copy),
        entry(Icon::Paste, "ST_MENU_PASTE", Action::Paste, MenuItem::Paste),
    ];
    ("ST_MENU_SECTION_CLIPBOARD", entries)
}

/// Rows commands, counting the selected `rows`.
fn rows_section(rows: usize) -> Section {
    let entries = vec![
        (
            Icon::InsertRows,
            count(
                rows,
                "ST_MENU_INSERT_ROW_ABOVE",
                "ST_MENU_INSERT_N_ROWS_ABOVE",
            ),
            "Ctrl++",
            MenuItem::InsertRowsAbove,
        ),
        (
            Icon::InsertRows,
            count(
                rows,
                "ST_MENU_INSERT_ROW_BELOW",
                "ST_MENU_INSERT_N_ROWS_BELOW",
            ),
            "",
            MenuItem::InsertRowsBelow,
        ),
        (
            Icon::FilterPlus,
            count(
                rows,
                "ST_MENU_KEEP_ONLY_THIS_ROW",
                "ST_MENU_KEEP_ONLY_THESE_N_ROWS",
            ),
            "",
            MenuItem::KeepRows,
        ),
        (
            Icon::Trash,
            count(rows, "ST_MENU_DELETE_ROW", "ST_MENU_DELETE_N_ROWS"),
            "Ctrl+-",
            MenuItem::DeleteRows,
        ),
    ];
    ("ST_MENU_SECTION_ROWS", entries)
}

/// A struct, list or map line also adds inside itself.
fn inside(line: Option<NodeKind>) -> Option<Section> {
    let (title, label) = match line? {
        NodeKind::Struct => ("ST_MENU_SECTION_STRUCT", "ST_MENU_ADD_FIELD"),
        NodeKind::List => ("ST_MENU_SECTION_LIST", "ST_MENU_ADD_ITEM_AT_THE_END"),
        NodeKind::Map => ("ST_MENU_SECTION_MAP", "ST_MENU_ADD_ENTRY_AT_THE_END"),
        _ => return None,
    };
    Some((
        title,
        vec![(Icon::Plus, tr(label), "", MenuItem::AddInside)],
    ))
}

/// Width of the right-click menu.
pub const MENU_WIDTH: f32 = 250.0;

/// Approximate height of a menu with these sections, to keep it on screen.
pub fn menu_height(sections: &[Section], ui: Ui) -> f32 {
    let entries: usize = sections.iter().map(|(_, e)| e.len()).sum();
    let item = ui.small() * 1.3 + 10.0 + 1.0;
    let title = ui.small() * 1.3 + 8.0;
    entries as f32 * item + sections.len() as f32 * (title + 7.0) + 10.0
}

/// Right-click menu for cells, rows or columns. `rows` is how many rows are
/// selected.
pub fn context_menu<'a>(sections: Vec<Section>, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
    let mut list = column![].spacing(1);
    let count = sections.len();
    for (i, (title, entries)) in sections.into_iter().enumerate() {
        list = list.push(
            container(
                text(tr(title))
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
                        text(tr(shortcut))
                            .size(ui.small() - 1.0)
                            .color(t.muted_text),
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

/// "ST_MENU_ADD_FIELD" for a struct column, "Add item/entry at the end" for a list
/// or map column. The rest of the Nested and Split & Merge commands stay on
/// the ribbon: the menu keeps to the basic, most used ones.
fn add_inside(column_type: Option<&DataType>) -> Option<MenuEntry> {
    let (label, item) = match column_type? {
        DataType::Struct(_) => ("ST_MENU_ADD_FIELD", MenuItem::AddField),
        DataType::List(_) | DataType::LargeList(_) => {
            ("ST_MENU_ADD_ITEM_AT_THE_END", MenuItem::AddInside)
        }
        DataType::Map(..) => ("ST_MENU_ADD_ENTRY_AT_THE_END", MenuItem::AddInside),
        _ => return None,
    };
    Some((Icon::Plus, tr(label), "", item))
}

/// An action's shortcut label, as the ribbon shows it.
fn shortcut(action: Action) -> &'static str {
    action.shortcut().unwrap_or_default()
}
