//! The right-click menu.

use super::*;
use iced::widget::column;
use veta_core::i18n::{tr, tr_with};
use veta_core::values::node::NodeKind;

/// One entry of a right-click menu: icon, label, shortcut, action. The
/// shortcut is shown through `tr`: key names (`ST_KEY_ENTER`) are tags,
/// key combinations ("Ctrl+X") show as they are.
type MenuEntry = (Icon, String, &'static str, MenuItem);

/// The right-click menu's sections for `target`: a title and its entries.
/// `line` is the kind of the selected line of an open cell, if one is.
pub fn menu_sections(
    target: MenuTarget,
    rows: usize,
    columns: usize,
    column_type: Option<&DataType>,
    line: Option<NodeKind>,
) -> Vec<(&'static str, Vec<MenuEntry>)> {
    // Labels, titles and shortcut key names are tags.
    let entry = |icon, label: &str, shortcut, item| (icon, tr(label), shortcut, item);
    let count = |n: usize, one: &str, many: &str| {
        if n == 1 {
            tr(one)
        } else {
            tr_with(many, &[("n", &n)])
        }
    };
    let mut sections: Vec<(&'static str, Vec<MenuEntry>)> = Vec::new();
    if target == MenuTarget::Blank {
        sections.push((
            "ST_MENU_SECTION_ADD",
            vec![
                entry(
                    Icon::InsertRows,
                    "ST_MENU_ADD_ROW",
                    "Ctrl++",
                    MenuItem::AddRow,
                ),
                entry(Icon::Plus, "ST_MENU_ADD_COLUMN", "", MenuItem::AddColumn),
            ],
        ));
        return sections;
    }
    // A struct, list or map line also adds inside itself.
    let inside = |sections: &mut Vec<(&'static str, Vec<MenuEntry>)>| match line {
        Some(NodeKind::Struct) => {
            sections.push((
                "ST_MENU_SECTION_STRUCT",
                vec![entry(
                    Icon::Plus,
                    "ST_MENU_ADD_FIELD",
                    "",
                    MenuItem::AddInside,
                )],
            ));
        }
        Some(NodeKind::List) => {
            sections.push((
                "ST_MENU_SECTION_LIST",
                vec![entry(
                    Icon::Plus,
                    "ST_MENU_ADD_ITEM_AT_THE_END",
                    "",
                    MenuItem::AddInside,
                )],
            ));
        }
        Some(NodeKind::Map) => {
            sections.push((
                "ST_MENU_SECTION_MAP",
                vec![entry(
                    Icon::Plus,
                    "ST_MENU_ADD_ENTRY_AT_THE_END",
                    "",
                    MenuItem::AddInside,
                )],
            ));
        }
        _ => {}
    };
    if target == MenuTarget::Field {
        sections.push((
            "ST_MENU_SECTION_STRUCT_FIELD",
            vec![
                entry(
                    Icon::Rename,
                    "ST_MENU_EDIT_FIELD",
                    "ST_KEY_ENTER",
                    MenuItem::EditCell,
                ),
                entry(
                    Icon::Eraser,
                    "ST_MENU_SET_TO_NULL",
                    "ST_KEY_DELETE",
                    MenuItem::ClearCell,
                ),
                entry(Icon::Plus, "ST_MENU_ADD_FIELD", "", MenuItem::AddField),
                entry(
                    Icon::Trash,
                    "ST_MENU_REMOVE_FIELD",
                    "",
                    MenuItem::RemoveField,
                ),
            ],
        ));
        inside(&mut sections);
        return sections;
    }
    if target == MenuTarget::Entry {
        sections.push((
            "ST_MENU_SECTION_MAP_ENTRY",
            vec![
                entry(
                    Icon::Rename,
                    "ST_MENU_EDIT_VALUE",
                    "ST_KEY_ENTER",
                    MenuItem::EditCell,
                ),
                entry(Icon::Rename, "ST_MENU_EDIT_KEY", "", MenuItem::EditKey),
                entry(
                    Icon::InsertRows,
                    "ST_MENU_INSERT_ENTRY_ABOVE",
                    "Ctrl++",
                    MenuItem::InsertItem,
                ),
                entry(
                    Icon::InsertRows,
                    "ST_MENU_INSERT_ENTRY_BELOW",
                    "",
                    MenuItem::InsertItemBelow,
                ),
                entry(
                    Icon::Trash,
                    "ST_MENU_DELETE_ENTRY",
                    "Ctrl+-",
                    MenuItem::DeleteItem,
                ),
                entry(
                    Icon::Eraser,
                    "ST_MENU_SET_VALUE_TO_NULL",
                    "ST_KEY_DELETE",
                    MenuItem::ClearCell,
                ),
            ],
        ));
        inside(&mut sections);
        return sections;
    }
    if target == MenuTarget::Item {
        sections.push((
            "ST_MENU_SECTION_LIST_ITEM",
            vec![
                entry(
                    Icon::Rename,
                    "ST_MENU_EDIT_ITEM",
                    "ST_KEY_ENTER",
                    MenuItem::EditCell,
                ),
                entry(
                    Icon::InsertRows,
                    "ST_MENU_INSERT_ITEM_ABOVE",
                    "Ctrl++",
                    MenuItem::InsertItem,
                ),
                entry(
                    Icon::InsertRows,
                    "ST_MENU_INSERT_ITEM_BELOW",
                    "",
                    MenuItem::InsertItemBelow,
                ),
                entry(
                    Icon::Trash,
                    "ST_MENU_DELETE_ITEM",
                    "Ctrl+-",
                    MenuItem::DeleteItem,
                ),
                entry(
                    Icon::Eraser,
                    "ST_MENU_SET_TO_NULL",
                    "ST_KEY_DELETE",
                    MenuItem::ClearCell,
                ),
            ],
        ));
        inside(&mut sections);
        return sections;
    }
    sections.push((
        "ST_MENU_SECTION_CLIPBOARD",
        vec![
            entry(Icon::Scissors, "ST_MENU_CUT", "Ctrl+X", MenuItem::Cut),
            entry(Icon::Copy, "ST_MENU_COPY", "Ctrl+C", MenuItem::Copy),
            entry(Icon::Paste, "ST_MENU_PASTE", "Ctrl+V", MenuItem::Paste),
        ],
    ));
    if target == MenuTarget::Cell {
        let mut cell = vec![
            entry(
                Icon::Rename,
                "ST_MENU_EDIT_CELL",
                "ST_KEY_ENTER",
                MenuItem::EditCell,
            ),
            entry(
                Icon::Eraser,
                "ST_MENU_SET_TO_NULL",
                "ST_KEY_DELETE",
                MenuItem::ClearCell,
            ),
        ];
        cell.extend(add_inside(column_type));
        sections.push(("ST_MENU_SECTION_CELL", cell));
        sections.push((
            "ST_MENU_SECTION_FILTER",
            vec![
                entry(
                    Icon::Filter,
                    "ST_MENU_FILTER_BY_THIS_VALUE",
                    "",
                    MenuItem::KeepValue,
                ),
                entry(
                    Icon::Filter,
                    "ST_MENU_FILTER_OUT_THIS_VALUE",
                    "",
                    MenuItem::ExcludeValue,
                ),
            ],
        ));
    }
    if matches!(target, MenuTarget::Cell | MenuTarget::Rows) {
        sections.push((
            "ST_MENU_SECTION_ROWS",
            vec![
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
            ],
        ));
    }
    if let MenuTarget::Column(_) = target {
        let mut column = vec![
            entry(
                Icon::Plus,
                "ST_MENU_INSERT_COLUMN_LEFT",
                "",
                MenuItem::InsertColumnLeft,
            ),
            entry(
                Icon::Plus,
                "ST_MENU_INSERT_COLUMN_RIGHT",
                "",
                MenuItem::InsertColumnRight,
            ),
            entry(Icon::Rename, "ST_MENU_RENAME", "", MenuItem::RenameColumn),
            entry(
                Icon::Copy,
                "ST_MENU_DUPLICATE",
                "",
                MenuItem::DuplicateColumn,
            ),
            entry(
                Icon::ArrowLeft,
                "ST_MENU_MOVE_LEFT",
                "",
                MenuItem::MoveColumnLeft,
            ),
            entry(
                Icon::ArrowRight,
                "ST_MENU_MOVE_RIGHT",
                "",
                MenuItem::MoveColumnRight,
            ),
            (
                Icon::Columns,
                count(
                    columns,
                    "ST_MENU_KEEP_ONLY_THIS_COLUMN",
                    "ST_MENU_KEEP_ONLY_THESE_N_COLUMNS",
                ),
                "",
                MenuItem::KeepColumns,
            ),
            (
                Icon::Trash,
                count(columns, "ST_MENU_REMOVE_COLUMN", "ST_MENU_REMOVE_N_COLUMNS"),
                "",
                MenuItem::RemoveColumns,
            ),
        ];
        column.extend(add_inside(column_type));
        sections.push(("ST_MENU_SECTION_COLUMN", column));
        sections.push((
            "ST_MENU_SECTION_DATA",
            vec![
                entry(Icon::Filter, "ST_MENU_FILTER", "", MenuItem::FilterColumn),
                entry(
                    Icon::SortAsc,
                    "ST_MENU_SORT_ASCENDING",
                    "",
                    MenuItem::SortAscending,
                ),
                entry(
                    Icon::SortDesc,
                    "ST_MENU_SORT_DESCENDING",
                    "",
                    MenuItem::SortDescending,
                ),
                entry(Icon::Type, "ST_MENU_CHANGE_TYPE", "", MenuItem::ChangeType),
                entry(
                    Icon::Replace,
                    "ST_MENU_REPLACE_VALUES",
                    "",
                    MenuItem::ReplaceValues,
                ),
                entry(
                    Icon::CaseUpper,
                    "ST_MENU_FORMAT_TEXT",
                    "",
                    MenuItem::FormatText,
                ),
                entry(Icon::FillDown, "ST_MENU_FILL_DOWN", "", MenuItem::FillDown),
                entry(Icon::FillUp, "ST_MENU_FILL_UP", "", MenuItem::FillUp),
            ],
        ));
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

/// Right-click menu for cells, rows or columns. `rows` is how many rows are
/// selected.
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
