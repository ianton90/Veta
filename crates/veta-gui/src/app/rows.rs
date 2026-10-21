//! Inserting, removing and keeping rows.

use super::*;

impl App {
    /// Rows dialog messages.
    pub(crate) fn rows_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Rows(message) => {
                if let Some(Dialog::Rows { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, RowsMessage::Cancel);
                    if let Some(command) = dialog.update(message) {
                        let inserted = match &command {
                            Command::InsertRows { at, count } => Some(*at..at + count),
                            _ => None,
                        };
                        match self
                            .workbook
                            .get_mut(id)
                            .map(|d| controller::execute(d, command))
                        {
                            Some(Ok(())) => {
                                self.dialog = None;
                                self.refresh(id);
                                if let (Some(rows), Some(doc), Some(tab)) = (
                                    inserted,
                                    self.workbook.get(id),
                                    self.tabs.iter_mut().find(|t| t.id == id),
                                ) {
                                    tab.grid.select_rows(rows, doc);
                                }
                            }
                            Some(Err(e)) => {
                                if let Some(Dialog::Rows { dialog, .. }) = &mut self.dialog {
                                    dialog.set_error(e.to_string());
                                }
                            }
                            None => self.dialog = None,
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            _ => unreachable!("not a rows message"),
        }
        Task::none()
    }

    /// Rows ribbon actions.
    pub(crate) fn rows_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::InsertRowsAt => {
                if let Some(id) = self.active {
                    return self.open_rows_dialog(
                        id,
                        dialogs::RowAction::Insert,
                        "ST_DLG_ROWS_INSERT_TITLE",
                    );
                }
            }
            Action::KeepRows | Action::RemoveRows => {
                // With a filter on, keeps or removes the rows it shows;
                // otherwise rows by position.
                if let Some(id) = self.active {
                    let remove = action == Action::RemoveRows;
                    let filtered = self
                        .tabs
                        .iter()
                        .find(|t| t.id == id)
                        .is_some_and(|t| t.grid.filter().is_some());
                    if filtered {
                        self.apply_filter(id, remove);
                    } else if !self.editable(id) {
                        self.not_editable(id);
                    } else if remove {
                        return self.open_rows_dialog(
                            id,
                            dialogs::RowAction::Delete,
                            "ST_DLG_ROWS_REMOVE_TITLE",
                        );
                    } else {
                        return self.open_rows_dialog(
                            id,
                            dialogs::RowAction::Keep,
                            "ST_DLG_ROWS_KEEP_TITLE",
                        );
                    }
                }
            }
            _ => unreachable!("not a rows action"),
        }
        Task::none()
    }

    pub(crate) fn selected_rows(&self, id: DocumentId) -> Option<std::ops::Range<usize>> {
        self.tabs.iter().find(|t| t.id == id)?.grid.selected_rows()
    }

    /// Inserts as many empty rows as are selected, above or below the
    /// selection, and selects them.
    pub(crate) fn insert_rows(&mut self, id: DocumentId, below: bool) {
        self.commit_edit();
        let Some(rows) = self.selected_rows(id) else {
            self.append_row(id);
            return;
        };
        let at = if below { rows.end } else { rows.start };
        let count = rows.len();
        if self.execute(id, Command::InsertRows { at, count })
            && let (Some(doc), Some(tab)) = (
                self.workbook.get(id),
                self.tabs.iter_mut().find(|t| t.id == id),
            )
        {
            tab.grid.select_rows(at..at + count, doc);
        }
    }

    /// Adds one row at the end and selects it.
    pub(crate) fn append_row(&mut self, id: DocumentId) {
        self.commit_edit();
        let Some(at) = self.workbook.get(id).map(Document::num_rows) else {
            return;
        };
        if self.execute(id, Command::InsertRows { at, count: 1 })
            && let (Some(doc), Some(tab)) = (
                self.workbook.get(id),
                self.tabs.iter_mut().find(|t| t.id == id),
            )
        {
            tab.grid.select_rows(at..at + 1, doc);
        }
    }

    /// Opens the dialog to insert, delete or keep a number of rows.
    fn open_rows_dialog(
        &mut self,
        id: DocumentId,
        action: dialogs::RowAction,
        title: &'static str,
    ) -> Task<Message> {
        self.commit_edit();
        let num_rows = self.workbook.get(id).map_or(0, Document::num_rows);
        let selected = self.selected_rows(id);
        self.edit = None;
        self.dialog = Some(Dialog::Rows {
            document: id,
            dialog: RowsDialog::new(action, title, num_rows, selected),
        });
        Task::batch([
            iced::widget::operation::focus(dialogs::NAME_ID),
            iced::widget::operation::select_all(dialogs::NAME_ID),
        ])
    }

    /// Keeps only the selected rows.
    pub(crate) fn keep_rows(&mut self, id: DocumentId) {
        let (Some(rows), Some(total)) = (
            self.selected_rows(id),
            self.workbook.get(id).map(Document::num_rows),
        ) else {
            return;
        };
        let delete: Vec<_> = [0..rows.start, rows.end..total]
            .into_iter()
            .filter(|r| !r.is_empty())
            .collect();
        if !delete.is_empty() {
            self.edit = None;
            self.execute(id, Command::DeleteRows { rows: delete });
        }
    }

    pub(crate) fn delete_rows(&mut self, id: DocumentId) {
        self.edit = None;
        if let Some(rows) = self.selected_rows(id) {
            self.execute(id, Command::DeleteRows { rows: vec![rows] });
        }
    }
}
