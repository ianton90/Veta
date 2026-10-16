//! Editing cells and nested values (struct fields, list items, map
//! entries), and undo / redo.

use super::*;

impl App {
    /// Messages about editing cells and nested values.
    pub(crate) fn editing_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EditInput(text) => match &mut self.edit {
                Some(edit) => {
                    edit.text = text;
                    edit.error = None;
                }
                None => {
                    // Typing straight into the formula bar starts an edit.
                    if let Some(id) = self.active
                        && self.editable(id)
                    {
                        let task = self.start_edit(id, Some(text));
                        return task;
                    }
                }
            },
            Message::CommitEdit => {
                // A value entered in the last item of a list, or in the slot
                // for a new one, goes on to a new item at the end; an empty
                // new item ends.
                // Map entries go key, value, next key; an empty new key ends.
                let item = self.edit.as_ref().and_then(|e| e.item.clone());
                let empty = self.edit.as_ref().is_some_and(|e| e.text.is_empty());
                let key = self.edit.as_ref().is_some_and(|e| e.key);
                if self.commit_edit()
                    && let Some(id) = self.active
                {
                    match item {
                        Some(item) if key && item.index == item.len && !empty => {
                            self.select_item(id, Some(item.index));
                            return self.start_edit(id, None);
                        }
                        Some(item) if key && item.index < item.len => {
                            self.select_item(id, Some(item.index));
                        }
                        Some(item) if item.field.is_some() => {
                            self.grid_event(id, GridEvent::Navigate(Nav::Down));
                        }
                        Some(item) if item.index == item.len && empty => {
                            self.select_item(id, None);
                        }
                        Some(item) if item.index + 1 >= item.len => {
                            self.select_item(id, Some(usize::MAX));
                            return self.start_edit(id, None);
                        }
                        _ => self.grid_event(id, GridEvent::Navigate(Nav::Down)),
                    }
                    return iced::widget::operation::focus(GRID_ID);
                }
            }
            _ => unreachable!("not a editing message"),
        }
        Task::none()
    }

    /// Undo, redo and nested-value ribbon actions.
    pub(crate) fn editing_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::ExpandAll => {
                if let Some(id) = self.active {
                    self.nested_step(id, false, true);
                }
            }
            Action::Undo | Action::Redo => {
                if let Some(id) = self.active
                    && self
                        .workbook
                        .get(id)
                        .is_some_and(|d| matches!(d.status(), Status::Computing { .. }))
                {
                    // Cancel stops the pass (and undoes the change).
                    self.not_editable(id);
                    return Task::none();
                }
                self.edit = None;
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get_mut(id)
                {
                    let result = if action == Action::Undo {
                        Ok(controller::undo(doc))
                    } else {
                        controller::redo(doc)
                    };
                    match result {
                        Ok(true) => self.refresh(id),
                        Ok(false) => {}
                        Err(e) => self.errors.push(e.to_string()),
                    }
                }
            }
            Action::ExpandStruct | Action::ExplodeList => {
                if let Some(id) = self.active {
                    self.nested_step(id, action == Action::ExplodeList, false);
                }
            }
            _ => unreachable!("not a editing action"),
        }
        Task::none()
    }

    /// Runs a nested value command (a struct field or list item) on a
    /// document, without refreshing its grid.
    fn nested_command(&mut self, id: DocumentId, command: Command) -> Result<(), String> {
        let doc = self.workbook.get_mut(id).ok_or("closed")?;
        controller::execute(doc, command).map_err(|e| e.to_string())
    }

    /// Expands the selected struct column, or explodes the selected list
    /// column into rows.
    pub(crate) fn nested_step(&mut self, id: DocumentId, explode: bool, all_levels: bool) {
        let Some(column) = self.selected_column_names(id).into_iter().next() else {
            self.errors.push("Select a column first.".into());
            return;
        };
        let command = if explode {
            Command::ExplodeList { column }
        } else {
            Command::ExpandStruct { column, all_levels }
        };
        self.execute(id, command);
    }

    /// Opens the Add field dialog on the selected struct column.
    pub(crate) fn open_add_field(&mut self, id: DocumentId) -> Task<Message> {
        let column = self.selected_column_names(id).into_iter().next();
        let is_struct = column.as_ref().is_some_and(|c| {
            self.workbook.get(id).is_some_and(|d| {
                d.schema().field_with_name(c).is_ok_and(|f| {
                    matches!(
                        f.data_type(),
                        veta_core::arrow::datatypes::DataType::Struct(_)
                    )
                })
            })
        });
        match column {
            Some(column) if is_struct => {
                self.open_column_dialog(id, ColumnDialog::add_field(column))
            }
            _ => {
                self.errors.push("Select a struct column first.".into());
                Task::none()
            }
        }
    }

    /// Removes the selected field of an open struct cell from its column.
    pub(crate) fn remove_field(&mut self, id: DocumentId) {
        let Some(item) = self.selected_list_item(id) else {
            return;
        };
        let Some(name) = item.field else {
            return;
        };
        if !item.list.is_empty() {
            self.errors
                .push("Only fields of top-level struct columns can be removed.".into());
            return;
        }
        self.execute(
            id,
            Command::RemoveField {
                column: item.column,
                name,
            },
        );
    }

    /// Row and column name of the selected cell in a document's grid.
    pub(crate) fn selected_cell(&self, id: DocumentId) -> Option<(usize, String)> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (row, column) = tab.grid.selected()?;
        let doc = self.workbook.get(id)?;
        Some((row, doc.schema().field(column).name().clone()))
    }

    /// Selects item `item` of the selected list cell (opening its row), or
    /// the cell itself.
    fn select_item(&mut self, id: DocumentId, item: Option<usize>) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.select_item(item, doc);
        }
    }

    /// The selected item of an open list cell, if one is.
    pub(crate) fn selected_list_item(&self, id: DocumentId) -> Option<ListItem> {
        let (row, column) = self.selected_cell(id)?;
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let item = tab.grid.selected_item()?;
        let list = tab.grid.selected_field()?.0.to_vec();
        Some(ListItem {
            row,
            column,
            list,
            index: item.index,
            len: item.len,
            data_type: item.data_type,
            field: item.field,
            key: item.key,
        })
    }

    /// Changes the list holding `item`.
    pub(crate) fn change_list(
        &mut self,
        id: DocumentId,
        item: &ListItem,
        change: &veta_core::values::ListEdit,
    ) -> Result<(), String> {
        use veta_core::display::PathPart;
        use veta_core::values::ListEdit;
        let (row, column) = (item.row, item.column.clone());
        let path: Vec<PathPart> = item.list.iter().cloned().map(PathPart::Field).collect();
        let child = |i: usize| {
            let mut p = path.clone();
            p.push(match (&item.field, item.key.is_some()) {
                (Some(f), _) => PathPart::Field(f.clone()),
                (None, true) => PathPart::Entry(i),
                (None, false) => PathPart::Index(i),
            });
            p
        };
        let command = match *change {
            ListEdit::Set { index, text, .. } => Command::SetValue {
                row,
                column,
                path: child(index),
                value: text.map(str::to_owned),
            },
            _ if item.field.is_some() => {
                return Err("struct fields can't be inserted or removed here".into());
            }
            ListEdit::Insert(_) if item.key.is_some() => {
                return Err("map entries need a key".into());
            }
            ListEdit::Insert(index) => Command::InsertItem {
                row,
                column,
                path,
                index,
                key: None,
            },
            ListEdit::Remove(index) => Command::RemoveItem {
                row,
                column,
                path,
                index,
            },
        };
        self.nested_command(id, command)
    }

    /// Renames the key of map entry `item`, or adds an entry with that key
    /// when `item` is the new-entry slot.
    fn change_map_key(&mut self, id: DocumentId, item: &ListItem, key: &str) -> Result<(), String> {
        use veta_core::display::PathPart;
        let (row, column, index) = (item.row, item.column.clone(), item.index);
        let path: Vec<PathPart> = item.list.iter().cloned().map(PathPart::Field).collect();
        let key = key.to_owned();
        let command = if index < item.len {
            Command::SetKey {
                row,
                column,
                path,
                index,
                key,
            }
        } else {
            Command::InsertItem {
                row,
                column,
                path,
                index,
                key: Some(key),
            }
        };
        self.nested_command(id, command)
    }

    /// Starts editing the key of the selected map entry.
    pub(crate) fn start_key_edit(&mut self, id: DocumentId) -> Task<Message> {
        let Some(item) = self.selected_list_item(id) else {
            return Task::none();
        };
        let Some(key) = item.key.clone() else {
            return Task::none();
        };
        self.edit = Some(Edit {
            document: id,
            row: item.row,
            column: item.column.clone(),
            field: None,
            text: key.unwrap_or_default(),
            item: Some(item),
            key: true,
            error: None,
        });
        Task::batch([
            iced::widget::operation::focus(EDITOR_ID),
            iced::widget::operation::move_cursor_to_end(EDITOR_ID),
        ])
    }

    /// Selects the new-entry slot of the selected map cell and starts
    /// editing the new key.
    pub(crate) fn add_map_entry(&mut self, id: DocumentId) -> Task<Message> {
        self.select_item(id, Some(usize::MAX));
        self.start_key_edit(id)
    }

    /// Inserts a null item above or below the selected item, or removes it,
    /// and selects the new item (or the one now in its place).
    pub(crate) fn insert_or_remove_item(
        &mut self,
        id: DocumentId,
        item: &ListItem,
        change: ItemChange,
    ) -> Task<Message> {
        use veta_core::values::ListEdit;
        if item.field.is_some() {
            return Task::none();
        }
        // A map entry needs a key first: inserting adds one at the end.
        if item.key.is_some() && change != ItemChange::Remove {
            return self.add_map_entry(id);
        }
        let (change, select) = match change {
            ItemChange::InsertAbove => {
                let at = item.index.min(item.len);
                (ListEdit::Insert(at), at)
            }
            ItemChange::InsertBelow => {
                let at = (item.index + 1).min(item.len);
                (ListEdit::Insert(at), at)
            }
            ItemChange::Remove if item.index < item.len => {
                let len = item.len - 1;
                (
                    ListEdit::Remove(item.index),
                    item.index.min(len.saturating_sub(1)),
                )
            }
            ItemChange::Remove => return Task::none(),
        };
        match self.change_list(id, item, &change) {
            Ok(()) => {
                self.refresh(id);
                self.select_item(id, Some(select));
            }
            Err(e) => self.errors.push(e),
        }
        Task::none()
    }

    /// The struct field shown in the selected cell, if it is one.
    pub(crate) fn selected_struct_field(&self, id: DocumentId) -> Option<StructField> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (path, data_type) = tab.grid.selected_field()?;
        (!path.is_empty()).then(|| StructField {
            path: path.to_vec(),
            data_type: data_type.clone(),
        })
    }

    /// Sets one field of a struct cell (`None` or empty: null).
    pub(crate) fn set_field(
        &mut self,
        id: DocumentId,
        row: usize,
        column: &str,
        field: &StructField,
        text: Option<&str>,
    ) -> Result<(), String> {
        let command = Command::SetValue {
            row,
            column: column.to_owned(),
            path: field
                .path
                .iter()
                .cloned()
                .map(veta_core::display::PathPart::Field)
                .collect(),
            value: text.map(str::to_owned),
        };
        self.nested_command(id, command)
    }

    /// Starts editing the selected cell in the formula bar.
    pub(crate) fn start_edit(&mut self, id: DocumentId, initial: Option<String>) -> Task<Message> {
        let Some((row, column)) = self.selected_cell(id) else {
            return Task::none();
        };
        // Lists are edited item by item: Enter edits the first item, typing
        // starts a new one at the end.
        if let Some(tab) = self.tabs.iter().find(|t| t.id == id)
            && let Some(kind) = tab.grid.selected_opens_items()
            && tab.grid.selected_item().is_none()
        {
            let item = match kind {
                crate::grid::OpenKind::List | crate::grid::OpenKind::Map if initial.is_some() => {
                    usize::MAX
                }
                _ => 0,
            };
            self.select_item(id, Some(item));
        }
        let item = self.selected_list_item(id);
        // The new-entry slot of a map starts with its key.
        if let Some(item) = &item
            && item.key == Some(None)
        {
            let task = self.start_key_edit(id);
            if let (Some(edit), Some(text)) = (&mut self.edit, initial) {
                edit.text = text;
            }
            return task;
        }
        let text = initial.unwrap_or_else(|| self.cell_text(id).unwrap_or_default());
        let field = if item.is_some() {
            None
        } else {
            self.selected_struct_field(id)
        };
        self.edit = Some(Edit {
            document: id,
            row,
            column,
            field,
            item,
            key: false,
            text,
            error: None,
        });
        Task::batch([
            iced::widget::operation::focus(EDITOR_ID),
            iced::widget::operation::move_cursor_to_end(EDITOR_ID),
        ])
    }

    /// Text of the selected cell (empty for null).
    pub(crate) fn cell_text(&self, id: DocumentId) -> Option<String> {
        Some(self.cell_value(id)?.unwrap_or_default())
    }

    /// Text of the selected cell (`None` inside for null).
    pub(crate) fn cell_value(&self, id: DocumentId) -> Option<Option<String>> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (row, column) = tab.grid.selected()?;
        match tab.grid.selected_edit_value() {
            Some(value) => Some(value.map(str::to_owned)),
            None if self.selected_struct_field(id).is_some() => None,
            None => {
                let batch = self.workbook.get(id)?.read(row..row + 1).ok()?;
                let cells = veta_core::display::format_batch(&batch).ok()?;
                Some(cells.first()?.get(column)?.clone())
            }
        }
    }

    /// Applies the formula bar edit, if any. Returns whether it was applied.
    ///
    /// Invalid input keeps the edit open with the error shown next to it.
    pub(crate) fn commit_edit(&mut self) -> bool {
        let Some(mut edit) = self.edit.take() else {
            return false;
        };
        let unchanged = match &edit.item {
            Some(item) if edit.key => {
                item.key.as_ref().map(|k| k.as_deref().unwrap_or_default())
                    == Some(edit.text.as_str())
            }
            _ => self.cell_text(edit.document).as_deref() == Some(edit.text.as_str()),
        };
        if unchanged {
            return true;
        }
        let result = if let Some(item) = edit.item.as_ref().filter(|_| edit.key) {
            self.change_map_key(edit.document, item, &edit.text)
        } else if let Some(item) = &edit.item {
            let text = (!edit.text.is_empty()).then_some(edit.text.as_str());
            let change = veta_core::values::ListEdit::Set {
                index: item.index,
                text,
                item_type: &item.data_type,
            };
            self.change_list(edit.document, item, &change)
        } else if let Some(field) = &edit.field {
            let text = (!edit.text.is_empty()).then_some(edit.text.as_str());
            self.set_field(edit.document, edit.row, &edit.column, field, text)
        } else {
            let command = Command::SetCell {
                row: edit.row,
                column: edit.column.clone(),
                value: Some(edit.text.clone()),
            };
            match self.workbook.get_mut(edit.document) {
                Some(doc) => controller::execute(doc, command).map_err(|e| e.to_string()),
                None => return false,
            }
        };
        match result {
            Ok(()) => {
                self.refresh(edit.document);
                true
            }
            Err(e) => {
                edit.error = Some(e);
                self.edit = Some(edit);
                false
            }
        }
    }

    /// Applies the edit before the selection moves away; if it can't be
    /// applied it is dropped and the error shown as a banner.
    pub(crate) fn commit_edit_or_drop(&mut self) {
        if !self.commit_edit()
            && let Some(edit) = self.edit.take()
            && let Some(error) = edit.error
        {
            self.errors.push(error);
        }
    }
}
