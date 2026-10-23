//! The ribbon: tabs of grouped commands (Home, Rows, Columns, Nested,
//! View). Every tab is always shown; commands that don't apply to the
//! selection are greyed out.
//!
//! Every command is an [`Action`]; keyboard shortcuts emit the same actions.
//! Commands that are not built yet are shown disabled, with the issue that
//! tracks them in the tooltip.
//!
//! Here: the actions, tabs, the ribbon's state and what is enabled;
//! `layout.rs`: each tab's groups; `view.rs`: drawing.

use crate::settings::config::ModePreference;
use crate::settings::theme::Tokens;

mod layout;
mod view;

pub use view::view;

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
    ZoomIn,
    ZoomOut,
    ZoomReset,
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

#[cfg(test)]
mod tests {
    use super::layout::groups;
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
