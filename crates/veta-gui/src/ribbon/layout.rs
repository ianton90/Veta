//! The ribbon's layout: each tab's groups of large and small commands.

use super::{Action, Tab};
use crate::platform::icon::Icon;
use crate::settings::config::ModePreference;

#[derive(Debug, Clone, Copy)]
pub(super) enum Size {
    Large,
    Small,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Item {
    pub(super) action: Action,
    /// Tag of the label (see `crate::i18n`).
    pub(super) label: &'static str,
    pub(super) icon: Icon,
    pub(super) size: Size,
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

pub(super) struct Group {
    /// Tag of the label.
    pub(super) label: &'static str,
    pub(super) items: Vec<Item>,
}

fn group(label: &'static str, items: Vec<Item>) -> Group {
    Group { label, items }
}

pub(super) fn groups(tab: Tab) -> Vec<Group> {
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
                "ST_RIBBON_GROUP_ZOOM",
                vec![
                    small(A::ZoomIn, "ST_RIBBON_BTN_ZOOM_IN", Icon::ZoomIn),
                    small(A::ZoomOut, "ST_RIBBON_BTN_ZOOM_OUT", Icon::ZoomOut),
                    small(A::ZoomReset, "ST_RIBBON_BTN_ZOOM_RESET", Icon::ZoomReset),
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
