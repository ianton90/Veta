//! Ribbon actions: [`App::perform`] hands each to its area; general ones
//! (panes, theme, settings) are handled here.

use super::*;

impl App {
    pub(crate) fn perform(&mut self, action: Action) -> Task<Message> {
        let changes_data = matches!(
            action,
            Action::InsertRowsAt
                | Action::AddColumn
                | Action::RenameColumn
                | Action::RemoveColumns
                | Action::KeepColumns
                | Action::SortAscending
                | Action::SortDescending
                | Action::CustomSort
                | Action::ExpandStruct
                | Action::ExplodeList
                | Action::DataType
                | Action::ReplaceValues
                | Action::FillDown
                | Action::FormatText
                | Action::SplitColumn
                | Action::MergeColumns
                | Action::CombineStruct
                | Action::CollectList
                | Action::CustomColumn
                | Action::DuplicateColumn
                | Action::IndexColumn
                | Action::Standard
                | Action::DateTime
                | Action::Cut
                | Action::Paste
                | Action::FillUp
                | Action::MoveColumnLeft
                | Action::MoveColumnRight
                | Action::ExpandAll
        );
        if changes_data
            && let Some(id) = self.active
            && !self.editable(id)
        {
            self.not_editable(id);
            return Task::none();
        }
        match action {
            a @ (Action::Copy | Action::Cut | Action::Paste) => return self.clipboard_action(a),
            a @ (Action::FillUp
            | Action::MoveColumnLeft
            | Action::MoveColumnRight
            | Action::ReplaceValues
            | Action::FillDown
            | Action::FormatText
            | Action::CustomColumn
            | Action::Standard
            | Action::DateTime
            | Action::DuplicateColumn
            | Action::IndexColumn
            | Action::SplitColumn
            | Action::MergeColumns
            | Action::CombineStruct
            | Action::CollectList
            | Action::DataType
            | Action::AddColumn
            | Action::RenameColumn
            | Action::RemoveColumns
            | Action::KeepColumns) => return self.columns_action(a),
            a @ (Action::ExpandAll
            | Action::Undo
            | Action::Redo
            | Action::ExpandStruct
            | Action::ExplodeList) => return self.editing_action(a),
            a @ (Action::Import
            | Action::Export
            | Action::New
            | Action::LoadRecipe
            | Action::SaveRecipe
            | Action::Open
            | Action::Close
            | Action::Save
            | Action::SaveAs
            | Action::FileMetadata
            | Action::WriterSettings) => return self.documents_action(a),
            a @ (Action::FilterRows
            | Action::CustomSort
            | Action::SortAscending
            | Action::SortDescending) => return self.filter_action(a),
            a @ (Action::InsertRowsAt | Action::KeepRows | Action::RemoveRows) => {
                return self.rows_action(a);
            }
            Action::ToggleDetails => self.show_side_pane = !self.show_side_pane,
            Action::AppliedSteps => {
                self.show_side_pane = true;
                self.side_tab = SideTab::Steps;
            }
            Action::StatisticsPane | Action::Statistics => {
                self.show_side_pane = true;
                self.side_tab = SideTab::Stats;
            }
            Action::Mode(mode) => {
                self.config.appearance.mode = mode;
                self.save_config();
            }
            Action::Settings => self.dialog = Some(Dialog::Settings(Draft::new(&self.config))),
            Action::About => self.dialog = Some(Dialog::About),
        }
        Task::none()
    }
}
