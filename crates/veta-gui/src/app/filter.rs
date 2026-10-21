//! The temporary filter view, filter steps and sorting.

use super::*;

impl App {
    /// Filter and sort messages.
    pub(crate) fn filter_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ClearFilter => {
                if let Some(id) = self.active {
                    self.clear_filter(id);
                }
            }
            _ => unreachable!("not a filter message"),
        }
        Task::none()
    }

    /// Filter and sort ribbon actions.
    pub(crate) fn filter_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::FilterRows => {
                if let Some(id) = self.active {
                    return self.open_filter_dialog(id);
                }
            }
            Action::CustomSort => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    let columns: Vec<String> = doc
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| f.name().clone())
                        .collect();
                    let column = self
                        .selected_column_names(id)
                        .into_iter()
                        .next()
                        .or_else(|| columns.first().cloned())
                        .unwrap_or_default();
                    let sort = Sort {
                        keys: vec![SortKey {
                            column,
                            descending: false,
                            nulls_first: false,
                        }],
                    };
                    self.edit = None;
                    self.dialog = Some(Dialog::Step {
                        document: id,
                        dialog: Box::new(SortDialog::new(columns, &sort)),
                        replace: None,
                    });
                }
            }
            Action::SortAscending | Action::SortDescending => {
                if let Some(id) = self.active {
                    self.sort_selected(id, action == Action::SortDescending);
                }
            }
            _ => unreachable!("not a filter action"),
        }
        Task::none()
    }

    /// Opens the filter dialog on the selected column.
    pub(crate) fn open_filter_dialog(&mut self, id: DocumentId) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let nested = nested_columns(&doc.schema());
        let columns: Vec<String> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        let selected = self
            .selected_columns(id)
            .and_then(|c| columns.get(c.start).cloned());
        let current = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.grid.filter().cloned());
        let dialog = match current {
            Some(filter) => FilterDialog::edit(columns, &filter),
            None => FilterDialog::new(columns, selected),
        }
        .with_nested(nested);
        self.edit = None;
        self.dialog = Some(Dialog::Step {
            document: id,
            dialog: Box::new(dialog),
            replace: None,
        });
        iced::widget::operation::focus(dialogs::FILTER_VALUE_ID)
    }

    /// Filters the rows shown (not a step). The filter is computed in the
    /// background; the table is read-only until it is cleared or applied.
    pub(crate) fn set_view_filter(&mut self, id: DocumentId, filter: Filter) -> Result<(), String> {
        let Some(doc) = self.workbook.get(id) else {
            return Ok(());
        };
        if doc.status() != &Status::Ready {
            return Err(tr("ST_MSG_WAIT_STEPS"));
        }
        let view = doc.filter_view(filter.clone()).map_err(|e| e.to_string())?;
        self.commit_edit_or_drop();
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.set_filter(Some((filter, view)), doc);
        }
        Ok(())
    }

    pub(crate) fn clear_filter(&mut self, id: DocumentId) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.set_filter(None, doc);
        }
    }

    /// Turns the temporary filter into a step that keeps (or removes) the
    /// rows it shows, and clears it.
    pub(crate) fn apply_filter(&mut self, id: DocumentId, remove: bool) {
        let Some(filter) = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.grid.filter().cloned())
        else {
            return;
        };
        self.clear_filter(id);
        self.execute(
            id,
            Command::Filter(Filter {
                exclude: remove,
                ..filter
            }),
        );
    }

    /// Sorts by the selected column.
    pub(crate) fn sort_selected(&mut self, id: DocumentId, descending: bool) {
        let column = self.selected_columns(id).map(|c| c.start).or_else(|| {
            self.tabs
                .iter()
                .find(|t| t.id == id)?
                .grid
                .selected()
                .map(|s| s.1)
        });
        let Some(column) = column.and_then(|c| self.column_names(id, c..c + 1).pop()) else {
            self.errors.push(tr("ST_MSG_SELECT_COLUMN_SORT"));
            return;
        };
        let sort = Sort {
            keys: vec![SortKey {
                column,
                descending,
                nulls_first: false,
            }],
        };
        self.execute(id, Command::Sort(sort));
    }

    /// Keeps (or removes) the rows whose value in the selected cell's
    /// column equals that cell's.
    pub(crate) fn quick_filter(&mut self, id: DocumentId, keep: bool) {
        let Some((_, column)) = self.selected_cell(id) else {
            return;
        };
        let Some(value) = self.cell_value(id) else {
            return;
        };
        let op = match (&value, keep) {
            (Some(_), true) => FilterOp::Equals,
            (Some(_), false) => FilterOp::NotEquals,
            (None, true) => FilterOp::IsNull,
            (None, false) => FilterOp::IsNotNull,
        };
        let condition = Condition {
            column,
            op,
            value: value.unwrap_or_default(),
            case_sensitive: true,
        };
        let current = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.grid.filter().cloned());
        let filter = match current {
            Some(mut f) if f.match_all => {
                f.conditions.push(condition);
                f
            }
            _ => Filter {
                conditions: vec![condition],
                match_all: true,
                exclude: false,
            },
        };
        if let Err(e) = self.set_view_filter(id, filter) {
            self.errors.push(e);
        }
    }
}
