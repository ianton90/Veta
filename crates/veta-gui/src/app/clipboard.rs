//! Copy, cut and paste.

use super::*;

/// Most cells copied at once: copying builds the whole text in memory, and
/// system clipboards struggle with hundreds of MB.
const MAX_COPY_CELLS: usize = 1_000_000;

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
        // An item inside an open cell: just its value.
        if let Some(item) = self.selected_cell_item(id).filter(|i| !i.item.is_slot()) {
            return self.copy_item(id, &item, cut);
        }
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
        let cells = match &selection {
            Selection::None => 0,
            Selection::Cell(..) => 1,
            Selection::Rows(rows) => rows.len() * doc.num_columns(),
            Selection::Columns(cols) => doc.num_rows() * cols.len(),
        };
        if self.too_many_to_copy(cells) {
            return Task::none();
        }
        let doc = self.workbook.get(id).expect("checked above");
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
                self.errors
                    .push(tr_with("ST_MSG_COPY_FAILED", &[("error", &error_text(&e))]));
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

    /// Copies the value of `item` (an item, entry or field line of an open
    /// cell); with `cut`, then clears it as Delete does.
    fn copy_item(&mut self, id: DocumentId, item: &CellItem, cut: bool) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let text = doc
            .schema()
            .index_of(&item.column)
            .map_err(veta_core::Error::from)
            .and_then(|c| veta_core::clipboard::copy_value(doc, item.row, c, &item.path()));
        let text = match text {
            Ok(text) => text,
            Err(e) => {
                self.errors
                    .push(tr_with("ST_MSG_COPY_FAILED", &[("error", &error_text(&e))]));
                return Task::none();
            }
        };
        if cut {
            match self.set_item_value(id, item, None) {
                Ok(()) => self.refresh(id),
                Err(e) => self.errors.push(e),
            }
        }
        iced::clipboard::write(text)
    }

    /// Refuses (with a message) to copy more than [`MAX_COPY_CELLS`] cells.
    fn too_many_to_copy(&mut self, cells: usize) -> bool {
        if cells <= MAX_COPY_CELLS {
            return false;
        }
        self.errors.push(tr_with(
            "ST_MSG_COPY_TOO_MANY",
            &[("n", &cells), ("max", &MAX_COPY_CELLS)],
        ));
        true
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
        if self.too_many_to_copy(rows.len() * targets.len()) {
            return Task::none();
        }
        let doc = self.workbook.get(id).expect("checked above");
        let text = match veta_core::clipboard::copy_fields(doc, rows, targets, header) {
            Ok(text) => text,
            Err(e) => {
                self.errors
                    .push(tr_with("ST_MSG_COPY_FAILED", &[("error", &error_text(&e))]));
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
                        .filter_map(|((_, path), column)| {
                            let (name, parents) = path.split_last()?;
                            Some(Command::RemoveField {
                                column,
                                path: parents
                                    .iter()
                                    .cloned()
                                    .map(veta_core::display::PathPart::Field)
                                    .collect(),
                                name: name.clone(),
                            })
                        })
                        .collect();
                    if commands.len() < targets.len() {
                        self.errors.push(tr("ST_MSG_CUT_FIELDS"));
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
                self.errors.push(tr("ST_MSG_PASTE_WHERE"));
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
