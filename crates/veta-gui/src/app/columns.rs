//! Column commands and their dialogs.

use super::*;

impl App {
    /// Messages of the choose-columns dialog.
    pub(crate) fn columns_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Choose(message) => {
                if let Some(Dialog::Choose { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    match dialog.update(message.clone()) {
                        Some(Some(command)) => {
                            if self.execute(id, command) {
                                self.dialog = None;
                            }
                        }
                        Some(None) => self.dialog = None,
                        None if matches!(message, ChooseMessage::Cancel) => self.dialog = None,
                        None => {}
                    }
                }
            }
            _ => unreachable!("not a columns message"),
        }
        Task::none()
    }

    /// Column ribbon actions.
    pub(crate) fn columns_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::FillUp => {
                if let Some(id) = self.active {
                    self.fill_selected(id, true);
                }
            }
            Action::MoveColumnLeft | Action::MoveColumnRight => {
                if let Some(id) = self.active
                    && let Some(c) = self.selected_columns(id)
                {
                    let to = if action == Action::MoveColumnLeft {
                        c.start.checked_sub(1)
                    } else {
                        Some(c.start + 1)
                    };
                    if let Some(to) = to {
                        self.move_column(id, c.start, to);
                    }
                }
            }
            Action::ReplaceValues => {
                if let Some(id) = self.active {
                    return self.open_replace_dialog(id, None);
                }
            }
            Action::FillDown => {
                if let Some(id) = self.active {
                    self.fill_selected(id, false);
                }
            }
            Action::FormatText => {
                if let Some(id) = self.active {
                    return self.open_format_dialog(id, None);
                }
            }
            Action::CustomColumn => {
                if let Some(id) = self.active {
                    return self.open_custom_dialog(id, None, String::new());
                }
            }
            Action::DuplicateColumn => {
                if let Some(id) = self.active {
                    self.duplicate_column(id);
                }
            }
            Action::IndexColumn => {
                if let Some(id) = self.active {
                    let at = self.column_count(id);
                    let name = self.unique_name(id, &tr("ST_COLUMN_NAME_INDEX"));
                    self.execute(
                        id,
                        Command::CustomColumn(CustomColumn {
                            name,
                            expression: "row()".into(),
                            at,
                        }),
                    );
                }
            }
            Action::SplitColumn => {
                if let Some(id) = self.active {
                    return self.open_split_dialog(id, None);
                }
            }
            Action::MergeColumns => {
                if let Some(id) = self.active {
                    return self.open_merge_dialog(id, None);
                }
            }
            Action::CombineStruct => {
                if let Some(id) = self.active {
                    return self.open_combine_dialog(id, None);
                }
            }
            Action::CollectList => {
                if let Some(id) = self.active {
                    return self.open_collect_dialog(id, None);
                }
            }
            Action::DataType => {
                if let Some(id) = self.active {
                    return self.open_cast_dialog(id, None);
                }
            }
            Action::AddColumn => {
                if let Some(id) = self.active {
                    let at = self
                        .selected_columns(id)
                        .map_or_else(|| self.column_count(id), |c| c.end);
                    return self.open_column_dialog(id, ColumnDialog::add(at), None);
                }
            }
            Action::RenameColumn => {
                if let Some(id) = self.active {
                    return self.rename_column(id);
                }
            }
            Action::RemoveColumns | Action::KeepColumns => {
                if let Some(id) = self.active {
                    self.keep_or_remove_columns(id, action == Action::KeepColumns, false);
                }
            }
            _ => unreachable!("not a columns action"),
        }
        Task::none()
    }

    pub(crate) fn selected_columns(&self, id: DocumentId) -> Option<std::ops::Range<usize>> {
        self.tab(id)?.grid.selected_columns()
    }

    pub(crate) fn select_columns(&mut self, id: DocumentId, range: std::ops::Range<usize>) {
        if let Some(tab) = self.tab_mut(id) {
            tab.grid.select_columns(range);
        }
    }

    fn column_count(&self, id: DocumentId) -> usize {
        self.workbook.get(id).map_or(0, Document::num_columns)
    }

    pub(crate) fn column_names(
        &self,
        id: DocumentId,
        range: std::ops::Range<usize>,
    ) -> Vec<String> {
        let Some(doc) = self.workbook.get(id) else {
            return Vec::new();
        };
        let schema = doc.schema();
        schema.fields()
            [range.start.min(schema.fields().len())..range.end.min(schema.fields().len())]
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    /// Opens the custom column dialog (with `expression`), or the custom
    /// column step at `index`.
    pub(crate) fn open_custom_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, CustomColumn)>,
        expression: String,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let level = step.as_ref().map_or(doc.evaluated_steps(), |(i, _)| *i);
        let Some(schema) = doc.schema_at(level) else {
            return Task::none();
        };
        let custom = match &step {
            Some((_, c)) => c.clone(),
            None => CustomColumn {
                name: self.unique_name(id, &tr("ST_COLUMN_NAME_CUSTOM")),
                expression,
                at: self
                    .selected_columns(id)
                    .map_or(schema.fields().len(), |c| c.end),
            },
        };
        self.open_step_dialog(id, CustomDialog::new(schema, custom), step.map(|(i, _)| i));
        Task::batch([
            iced::widget::operation::focus(dialogs::EXPRESSION_ID),
            iced::widget::operation::move_cursor_to_end(dialogs::EXPRESSION_ID),
        ])
    }

    /// Adds a copy of the selected column next to it.
    pub(crate) fn duplicate_column(&mut self, id: DocumentId) {
        let Some(range) = self.selected_columns(id).or_else(|| {
            let (_, c) = self.tab(id)?.grid.selected()?;
            Some(c..c + 1)
        }) else {
            self.errors.push(tr("ST_MSG_SELECT_COLUMN_DUPLICATE"));
            return;
        };
        let Some(column) = self.column_names(id, range.start..range.start + 1).pop() else {
            return;
        };
        let name = self.unique_name(id, &tr_with("ST_COLUMN_NAME_COPY", &[("column", &column)]));
        self.execute(
            id,
            Command::CustomColumn(CustomColumn {
                name,
                expression: formula_reference(&column),
                at: range.start + 1,
            }),
        );
    }

    /// `base`, or `base 2`, `base 3`… if a column already has that name.
    fn unique_name(&self, id: DocumentId, base: &str) -> String {
        let Some(doc) = self.workbook.get(id) else {
            return base.to_owned();
        };
        let schema = doc.schema();
        let taken = |name: &str| schema.fields().iter().any(|f| f.name() == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Opens the split dialog on the selected column, or on the split step
    /// at `index`.
    pub(crate) fn open_split_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Split)>,
    ) -> Task<Message> {
        let column = match &step {
            Some((_, s)) => Some(s.column.clone()),
            None => self.selected_column_names(id).into_iter().next(),
        };
        let Some(column) = column else {
            self.errors.push(tr("ST_MSG_SELECT_COLUMN_SPLIT"));
            return Task::none();
        };
        let Some(sample) = self.sample(id, step.as_ref().map(|(i, _)| *i), &column) else {
            return Task::none();
        };
        self.open_step_dialog(
            id,
            SplitDialog::new(column, sample, step.as_ref().map(|(_, s)| s)),
            step.map(|(i, _)| i),
        );
        iced::widget::operation::focus(dialogs::NAME_ID)
    }

    /// Opens the merge dialog on the selected columns, or on the merge step
    /// at `index`.
    pub(crate) fn open_merge_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Merge)>,
    ) -> Task<Message> {
        let columns = match &step {
            Some((_, m)) => m.columns.clone(),
            None => self.selected_column_names(id),
        };
        if columns.len() < 2 {
            self.errors.push(tr("ST_MSG_SELECT_COLUMNS_MERGE"));
            return Task::none();
        }
        self.open_step_dialog(
            id,
            MergeDialog::new(columns, step.as_ref().map(|(_, m)| m)),
            step.map(|(i, _)| i),
        );
        select_name()
    }

    /// Opens the collect dialog on the selected columns, or on the collect
    /// step at `index`.
    pub(crate) fn open_collect_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Vec<String>, Vec<String>)>,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let (replace, keys, columns, schema) = match step {
            Some((i, keys, columns)) => (Some(i), keys, columns, doc.schema_at(i)),
            None => {
                let schema = doc.schema_at(doc.evaluated_steps());
                (None, Vec::new(), self.selected_column_names(id), schema)
            }
        };
        if columns.is_empty() {
            self.errors.push(tr("ST_MSG_SELECT_COLUMNS_COLLECT"));
            return Task::none();
        }
        let others = schema
            .map(|s| {
                s.fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .filter(|name| !columns.contains(name))
                    .map(|name| {
                        let on = keys.contains(&name);
                        (name, on)
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.open_step_dialog(id, CollectDialog::new(columns, others), replace);
        Task::none()
    }

    /// Opens the combine dialog on the selected columns, or on the combine
    /// step at `index`.
    pub(crate) fn open_combine_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, (Vec<String>, String))>,
    ) -> Task<Message> {
        let (replace, columns, name) = match step {
            Some((i, (columns, name))) => (Some(i), columns, name),
            None => {
                let columns = self.selected_column_names(id);
                // Columns from one expanded struct (`a.x`, `a.y`) go back
                // into `a`.
                let prefix = columns
                    .first()
                    .and_then(|c| c.rsplit_once('.'))
                    .map(|(p, _)| p.to_owned())
                    .filter(|p| {
                        columns.iter().all(|c| {
                            c.strip_prefix(p.as_str())
                                .is_some_and(|r| r.starts_with('.'))
                        })
                    });
                let name =
                    prefix.unwrap_or_else(|| self.unique_name(id, &tr("ST_COLUMN_NAME_STRUCT")));
                (None, columns, name)
            }
        };
        if columns.is_empty() {
            self.errors.push(tr("ST_MSG_SELECT_COLUMNS_COMBINE"));
            return Task::none();
        }
        self.open_step_dialog(id, CombineDialog::new(columns, name), replace);
        select_name()
    }

    /// The first rows of `column` after `level` steps (all evaluated steps
    /// when `None`).
    fn sample(&self, id: DocumentId, level: Option<usize>, column: &str) -> Option<ArrayRef> {
        let doc = self.workbook.get(id)?;
        let level = level.unwrap_or(doc.evaluated_steps());
        let batch = doc.read_at(level, 0..SAMPLE_ROWS).ok()?;
        batch.column_by_name(column).cloned()
    }

    /// Names of the selected columns, or of the selected cell's column.
    pub(crate) fn selected_column_names(&self, id: DocumentId) -> Vec<String> {
        let range = self.selected_columns(id).or_else(|| {
            let (_, c) = self.tab(id)?.grid.selected()?;
            Some(c..c + 1)
        });
        range.map_or_else(Vec::new, |r| self.column_names(id, r))
    }

    /// Opens the replace values dialog on the selected column, or on the
    /// replace step at `index`.
    pub(crate) fn open_replace_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Replace)>,
    ) -> Task<Message> {
        let column = match &step {
            Some((_, r)) => Some(r.column.clone()),
            None => self.selected_column_names(id).into_iter().next(),
        };
        let Some(column) = column else {
            self.errors.push(tr("ST_MSG_SELECT_COLUMN_REPLACE"));
            return Task::none();
        };
        let level = step.as_ref().map(|(i, _)| *i);
        let text_column = self
            .workbook
            .get(id)
            .and_then(|d| match level {
                Some(i) => d.schema_at(i),
                None => Some(d.schema()),
            })
            .and_then(|s| {
                let field = s.field_with_name(&column).ok()?;
                Some(veta_core::values::is_text(field.data_type()))
            })
            .unwrap_or(true);
        self.open_step_dialog(
            id,
            ReplaceDialog::new(column, text_column, step.as_ref().map(|(_, r)| r)),
            level,
        );
        iced::widget::operation::focus(dialogs::NAME_ID)
    }

    /// Opens the format text dialog on the selected columns, or on the
    /// format step at `index`.
    pub(crate) fn open_format_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Vec<String>, TextOp)>,
    ) -> Task<Message> {
        let (replace, columns, op) = match step {
            Some((i, columns, op)) => (Some(i), columns, op),
            None => (None, self.selected_column_names(id), TextOp::Trim),
        };
        if columns.is_empty() {
            self.errors.push(tr("ST_MSG_SELECT_COLUMNS_FORMAT"));
            return Task::none();
        }
        self.open_step_dialog(id, FormatDialog::new(columns, op), replace);
        Task::none()
    }

    /// Fills nulls in the selected columns.
    pub(crate) fn fill_selected(&mut self, id: DocumentId, up: bool) {
        let columns = self.selected_column_names(id);
        if columns.is_empty() {
            self.errors.push(tr("ST_MSG_SELECT_COLUMNS_FILL"));
            return;
        }
        self.execute(id, Command::Fill(Fill { columns, up }));
    }

    /// Opens the change type dialog on the selected column, or on the cast
    /// step at `index` (previewed on that step's input).
    pub(crate) fn open_cast_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Cast)>,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let level = step.as_ref().map_or(doc.evaluated_steps(), |(i, _)| *i);
        let column = match &step {
            Some((_, cast)) => Some(cast.column.clone()),
            None => {
                let index = self
                    .selected_columns(id)
                    .map(|c| c.start)
                    .or_else(|| self.tab(id)?.grid.selected().map(|s| s.1));
                index.and_then(|c| self.column_names(id, c..c + 1).pop())
            }
        };
        let Some(column) = column else {
            self.errors.push(tr("ST_MSG_SELECT_COLUMN_CAST"));
            return Task::none();
        };
        let Some(sample) = self.sample(id, Some(level), &column) else {
            return Task::none();
        };
        self.open_step_dialog(
            id,
            CastDialog::new(column, sample, step.as_ref().map(|(_, c)| c)),
            step.map(|(i, _)| i),
        );
        Task::none()
    }

    /// Opens a column dialog: adding or renaming a column or adding a
    /// field, or editing the step at `replace`.
    pub(crate) fn open_column_dialog(
        &mut self,
        id: DocumentId,
        dialog: ColumnDialog,
        replace: Option<usize>,
    ) -> Task<Message> {
        self.open_step_dialog(id, dialog, replace);
        select_name()
    }

    pub(crate) fn rename_column(&mut self, id: DocumentId) -> Task<Message> {
        let Some(columns) = self.selected_columns(id) else {
            return Task::none();
        };
        match self
            .column_names(id, columns.start..columns.start + 1)
            .pop()
        {
            Some(name) => self.open_column_dialog(id, ColumnDialog::rename(name), None),
            None => Task::none(),
        }
    }

    /// Keeps or removes columns. From the ribbon, with at most one column
    /// selected, a dialog asks which; otherwise the selected columns are
    /// used directly.
    pub(crate) fn keep_or_remove_columns(&mut self, id: DocumentId, keep: bool, from_menu: bool) {
        self.edit = None;
        let selected = self
            .selected_columns(id)
            .map(|c| self.column_names(id, c))
            .unwrap_or_default();
        if from_menu || selected.len() > 1 {
            if selected.is_empty() {
                return;
            }
            let command = if keep {
                Command::KeepColumns { names: selected }
            } else {
                Command::RemoveColumns { names: selected }
            };
            self.execute(id, command);
            return;
        }
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let names: Vec<String> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        self.dialog = Some(Dialog::Choose {
            document: id,
            dialog: ChooseColumns::new(keep, names, &selected),
        });
    }

    pub(crate) fn move_column(&mut self, id: DocumentId, from: usize, to: usize) {
        self.commit_edit();
        if let Some(name) = self.column_names(id, from..from + 1).pop()
            && self.execute(id, Command::MoveColumn { name, to })
        {
            self.select_columns(id, to..to + 1);
        }
    }
}

/// Focuses a dialog's name field with its text selected, to type over.
pub(crate) fn select_name() -> Task<Message> {
    Task::batch([
        iced::widget::operation::focus(dialogs::NAME_ID),
        iced::widget::operation::select_all(dialogs::NAME_ID),
    ])
}
