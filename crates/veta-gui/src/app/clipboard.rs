//! Copy, cut and paste.

use super::*;

impl App {
    /// Clipboard messages.
    pub(crate) fn clipboard_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Pasted(text) => {
                if let (Some(id), Some(text)) = (self.active, text) {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    self.paste(id, &text);
                }
            }
            _ => unreachable!("not a clipboard message"),
        }
        Task::none()
    }

    /// Clipboard ribbon actions.
    pub(crate) fn clipboard_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Copy | Action::Cut => {
                if let Some(id) = self.active {
                    return self.copy_selection(id, action == Action::Cut);
                }
            }
            Action::Paste => {
                if self.active.is_some() {
                    return iced::clipboard::read().map(Message::Pasted);
                }
            }
            _ => unreachable!("not a clipboard action"),
        }
        Task::none()
    }

    /// Copies the selection to the clipboard as tab-separated text (whole
    /// columns with their names first). With `cut`, then clears the cell or
    /// removes the rows or columns.
    pub(crate) fn copy_selection(&mut self, id: DocumentId, cut: bool) -> Task<Message> {
        self.commit_edit();
        let (Some(tab), Some(doc)) = (self.tabs.iter().find(|t| t.id == id), self.workbook.get(id))
        else {
            return Task::none();
        };
        let selection = tab.grid.selection();
        // Fields of expanded struct columns copy just those fields.
        let fields = tab
            .grid
            .selected_targets()
            .filter(|(_, fields)| *fields && !matches!(selection, Selection::Rows(_)))
            .map(|(targets, _)| targets);
        if let Some(targets) = fields {
            return self.copy_fields(id, &selection, &targets, cut);
        }
        let text = match &selection {
            Selection::None => return Task::none(),
            Selection::Cell(r, c) => {
                veta_core::clipboard::copy_text(doc, *r..r + 1, *c..c + 1, false)
            }
            Selection::Rows(rows) => {
                veta_core::clipboard::copy_text(doc, rows.clone(), 0..doc.num_columns(), false)
            }
            Selection::Columns(cols) => {
                veta_core::clipboard::copy_text(doc, 0..doc.num_rows(), cols.clone(), true)
            }
        };
        let text = match text {
            Ok(text) => text,
            Err(e) => {
                self.errors.push(format!("Could not copy: {e}"));
                return Task::none();
            }
        };
        if cut {
            match selection {
                Selection::Cell(row, c) => {
                    let column = doc.schema().field(c).name().clone();
                    self.execute(
                        id,
                        Command::SetCell {
                            row,
                            column,
                            value: None,
                        },
                    );
                }
                Selection::Rows(_) => self.delete_rows(id),
                Selection::Columns(_) => self.keep_or_remove_columns(id, false, true),
                Selection::None => {}
            }
        }
        iced::clipboard::write(text)
    }

    /// Copies fields of expanded struct columns (a cell or whole columns);
    /// with `cut`, then sets the field to null or removes the fields.
    fn copy_fields(
        &mut self,
        id: DocumentId,
        selection: &Selection,
        targets: &[(usize, Vec<String>)],
        cut: bool,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let (rows, header) = match selection {
            Selection::Cell(r, _) => (*r..r + 1, false),
            _ => (0..doc.num_rows(), true),
        };
        let text = match veta_core::clipboard::copy_fields(doc, rows, targets, header) {
            Ok(text) => text,
            Err(e) => {
                self.errors.push(format!("Could not copy: {e}"));
                return Task::none();
            }
        };
        if cut {
            let names: Vec<String> = targets
                .iter()
                .map(|(c, _)| doc.schema().field(*c).name().clone())
                .collect();
            match selection {
                Selection::Cell(row, _) => {
                    if let (Some(field), Some(name)) =
                        (self.selected_struct_field(id), names.first())
                    {
                        match self.set_field(id, *row, name, &field, None) {
                            Ok(()) => self.refresh(id),
                            Err(e) => self.errors.push(e),
                        }
                    }
                }
                _ => {
                    let commands: Vec<Command> = targets
                        .iter()
                        .zip(names)
                        .filter_map(|((_, path), column)| match path.as_slice() {
                            [name] => Some(Command::RemoveField {
                                column,
                                name: name.clone(),
                            }),
                            _ => None,
                        })
                        .collect();
                    if commands.len() < targets.len() {
                        self.errors
                            .push("Only fields of top-level struct columns can be cut.".into());
                    } else {
                        for command in commands {
                            if !self.execute(id, command) {
                                break;
                            }
                        }
                    }
                }
            }
        }
        iced::clipboard::write(text)
    }

    /// Pastes tab-separated `text` at the selected cell (or the start of
    /// the selected rows or columns) as one change.
    pub(crate) fn paste(&mut self, id: DocumentId, text: &str) {
        self.commit_edit();
        let (Some(tab), Some(doc)) = (self.tabs.iter().find(|t| t.id == id), self.workbook.get(id))
        else {
            return;
        };
        let mut values = veta_core::clipboard::parse_tsv(text);
        // Into fields of expanded struct columns.
        if let Some((targets, true)) = tab.grid.paste_targets() {
            let row = match tab.grid.selection() {
                Selection::Cell(r, _) => r,
                Selection::Columns(_) => {
                    let header: Vec<Option<String>> = targets
                        .iter()
                        .take(values.first().map_or(0, Vec::len))
                        .map(|(c, path)| {
                            let mut name = doc.schema().field(*c).name().clone();
                            for part in path {
                                name = format!("{name}.{part}");
                            }
                            Some(name)
                        })
                        .collect();
                    if values.first() == Some(&header) {
                        values.remove(0);
                    }
                    0
                }
                _ => return,
            };
            if !values.is_empty() {
                self.execute(
                    id,
                    Command::PasteFields {
                        row,
                        targets,
                        values,
                    },
                );
            }
            return;
        }
        let (row, column) = match tab.grid.selection() {
            Selection::None => {
                self.errors.push("Select where to paste first.".into());
                return;
            }
            Selection::Cell(r, c) => (r, c),
            Selection::Rows(rows) => (rows.start, 0),
            Selection::Columns(cols) => {
                // Copied whole columns start with their names.
                let names = self.column_names(id, cols.clone());
                let header: Vec<Option<String>> = names.into_iter().map(Some).collect();
                if values.first() == Some(&header) {
                    values.remove(0);
                }
                (0, cols.start)
            }
        };
        if values.is_empty() || doc.num_columns() == 0 {
            return;
        }
        self.execute(
            id,
            Command::PasteCells {
                row,
                column,
                values,
            },
        );
    }
}
