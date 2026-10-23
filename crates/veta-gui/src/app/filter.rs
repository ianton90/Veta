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
                    return self.open_filter_dialog(id, None);
                }
            }
            Action::CustomSort => {
                if let Some(id) = self.active {
                    return self.open_sort_dialog(id, None);
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

    /// Opens the filter dialog on the selected column (or the filter
    /// shown), or on the filter step at `index`.
    pub(crate) fn open_filter_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Filter)>,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let Some(schema) = level_schema(doc, step.as_ref().map(|(i, _)| *i)) else {
            return Task::none();
        };
        let columns = column_names(&schema);
        let (replace, filter) = match step {
            Some((i, filter)) => (Some(i), Some(filter)),
            None => (None, self.tab(id).and_then(|t| t.grid.filter().cloned())),
        };
        let dialog = match filter {
            Some(filter) => FilterDialog::edit(columns, &filter),
            None => {
                let selected = self
                    .selected_columns(id)
                    .and_then(|c| columns.get(c.start).cloned());
                FilterDialog::new(columns, selected)
            }
        }
        .with_nested(nested_columns(&schema));
        self.open_step_dialog(id, dialog, replace);
        iced::widget::operation::focus(dialogs::FILTER_VALUE_ID)
    }

    /// Opens the sort dialog on the selected column, or on the sort step at
    /// `index`.
    pub(crate) fn open_sort_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Sort)>,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let Some(schema) = level_schema(doc, step.as_ref().map(|(i, _)| *i)) else {
            return Task::none();
        };
        let columns = column_names(&schema);
        let (replace, sort) = match step {
            Some((i, sort)) => (Some(i), sort),
            None => {
                let column = self
                    .selected_column_names(id)
                    .into_iter()
                    .next()
                    .or_else(|| columns.first().cloned())
                    .unwrap_or_default();
                let key = SortKey {
                    column,
                    descending: false,
                    nulls_first: false,
                };
                (None, Sort { keys: vec![key] })
            }
        };
        self.open_step_dialog(id, SortDialog::new(columns, &sort), replace);
        Task::none()
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
        let view = doc
            .filter_view(filter.clone())
            .map_err(|e| error_text(&e))?;
        self.commit_edit_or_drop();
        self.grid_do(id, |grid, doc| grid.set_filter(Some((filter, view)), doc));
        Ok(())
    }

    pub(crate) fn clear_filter(&mut self, id: DocumentId) {
        self.grid_do(id, |grid, doc| grid.set_filter(None, doc));
    }

    /// Turns the temporary filter into a step that keeps (or removes) the
    /// rows it shows, and clears it.
    pub(crate) fn apply_filter(&mut self, id: DocumentId, remove: bool) {
        let Some(filter) = self.tab(id).and_then(|t| t.grid.filter().cloned()) else {
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
        let column = self
            .selected_columns(id)
            .map(|c| c.start)
            .or_else(|| self.tab(id)?.grid.selected().map(|s| s.1));
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
        let current = self.tab(id).and_then(|t| t.grid.filter().cloned());
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

/// The schema after `level` steps, or the current one.
fn level_schema(
    doc: &Document,
    level: Option<usize>,
) -> Option<veta_core::arrow::datatypes::SchemaRef> {
    match level {
        Some(i) => doc.schema_at(i),
        None => Some(doc.schema()),
    }
}

fn column_names(schema: &veta_core::arrow::datatypes::Schema) -> Vec<String> {
    schema.fields().iter().map(|f| f.name().clone()).collect()
}
