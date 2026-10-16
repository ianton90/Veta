//! The right-click menu.

use super::*;
use iced::widget::column;
use veta_core::values::node::NodeKind;

/// One entry of a right-click menu: icon, label, shortcut, action.
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
    let entry = |icon, label: &str, shortcut, item| (icon, label.to_owned(), shortcut, item);
    let count = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_owned()
        } else {
            many.replace("{n}", &n.to_string())
        }
    };
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
    // A struct, list or map line also adds inside itself.
    let inside = |sections: &mut Vec<(&'static str, Vec<MenuEntry>)>| match line {
        Some(NodeKind::Struct) => {
            sections.push((
                "Struct",
                vec![entry(Icon::Plus, "Add field…", "", MenuItem::AddInside)],
            ));
        }
        Some(NodeKind::List) => {
            sections.push((
                "List",
                vec![entry(
                    Icon::Plus,
                    "Add item at the end",
                    "",
                    MenuItem::AddInside,
                )],
            ));
        }
        Some(NodeKind::Map) => {
            sections.push((
                "Map",
                vec![entry(
                    Icon::Plus,
                    "Add entry at the end",
                    "",
                    MenuItem::AddInside,
                )],
            ));
        }
        _ => {}
    };
    if target == MenuTarget::Field {
        sections.push((
            "Struct field",
            vec![
                entry(Icon::Rename, "Edit field", "Enter", MenuItem::EditCell),
                entry(Icon::Eraser, "Set to null", "Delete", MenuItem::ClearCell),
                entry(Icon::Plus, "Add field…", "", MenuItem::AddField),
                entry(Icon::Trash, "Remove field", "", MenuItem::RemoveField),
            ],
        ));
        inside(&mut sections);
        return sections;
    }
    if target == MenuTarget::Entry {
        sections.push((
            "Map entry",
            vec![
                entry(Icon::Rename, "Edit value", "Enter", MenuItem::EditCell),
                entry(Icon::Rename, "Edit key", "", MenuItem::EditKey),
                entry(
                    Icon::InsertRows,
                    "Insert entry above",
                    "Ctrl++",
                    MenuItem::InsertItem,
                ),
                entry(
                    Icon::InsertRows,
                    "Insert entry below",
                    "",
                    MenuItem::InsertItemBelow,
                ),
                entry(Icon::Trash, "Delete entry", "Ctrl+-", MenuItem::DeleteItem),
                entry(
                    Icon::Eraser,
                    "Set value to null",
                    "Delete",
                    MenuItem::ClearCell,
                ),
            ],
        ));
        inside(&mut sections);
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
        inside(&mut sections);
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
        let cell = vec![
            entry(Icon::Rename, "Edit cell", "Enter", MenuItem::EditCell),
            entry(Icon::Eraser, "Set to null", "Delete", MenuItem::ClearCell),
        ];
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

/// Expand/explode entries for a column of `column_type`.
fn nested_items(
    column_type: Option<&DataType>,
) -> Vec<Option<(Icon, String, &'static str, MenuItem)>> {
    match column_type {
        Some(DataType::Struct(fields)) => {
            let mut items = vec![
                Some((
                    Icon::ListTree,
                    "Expand fields to columns".into(),
                    "",
                    MenuItem::ExpandStruct,
                )),
                Some((Icon::Plus, "Add field…".into(), "", MenuItem::AddField)),
            ];
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
        ) => vec![
            Some((
                Icon::Plus,
                if matches!(column_type, Some(DataType::Map(..))) {
                    "Add entry at the end".into()
                } else {
                    "Add item at the end".into()
                },
                "",
                MenuItem::AddInside,
            ))
            .filter(|_| !matches!(column_type, Some(DataType::FixedSizeList(..)))),
            Some((
                Icon::Rows,
                "Explode to rows".into(),
                "",
                MenuItem::ExplodeList,
            )),
        ],
        _ => Vec::new(),
    }
}
