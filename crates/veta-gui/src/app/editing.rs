//! Editing cells and nested values (struct fields, list items, the Value
//! tab and the JSON editor), and undo / redo.

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
                let item = self.edit.as_ref().and_then(|e| e.item.clone());
                let empty = self.edit.as_ref().is_some_and(|e| e.text.is_empty());
                if self.commit_edit()
                    && let Some(id) = self.active
                {
                    match item {
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
            Message::EditLeaf(pointer, data_type, text) => {
                if let Some(id) = self.active {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    let selected = self
                        .tabs
                        .iter()
                        .find(|t| t.id == id)
                        .and_then(|t| t.grid.selected());
                    if let Some((row, column)) = selected {
                        self.edit = None;
                        self.leaf_edit = Some(LeafEdit {
                            document: id,
                            row,
                            column,
                            pointer,
                            data_type,
                            text,
                            error: None,
                            rename: false,
                        });
                        return iced::widget::operation::focus(LEAF_ID);
                    }
                }
            }
            Message::EditKey(pointer, key) => {
                let selected = self.active.filter(|&id| self.editable(id)).and_then(|id| {
                    let tab = self.tabs.iter().find(|t| t.id == id)?;
                    Some((id, tab.grid.selected()?))
                });
                if let Some((id, (row, column))) = selected {
                    self.edit = None;
                    self.leaf_edit = Some(LeafEdit {
                        document: id,
                        row,
                        column,
                        pointer,
                        data_type: veta_core::arrow::datatypes::DataType::Utf8,
                        text: key,
                        error: None,
                        rename: true,
                    });
                    return iced::widget::operation::focus(LEAF_ID);
                }
            }
            Message::OpenJsonEditor => {
                if let Some(id) = self.active {
                    self.open_json_editor(id);
                }
            }
            Message::Json(message) => {
                if let Some(Dialog::Json { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, dialogs::JsonMessage::Cancel);
                    if let Some(value) = dialog.update(message) {
                        let command = Command::SetCell {
                            row: dialog.row,
                            column: dialog.column.clone(),
                            value,
                        };
                        let result = self
                            .workbook
                            .get_mut(id)
                            .map(|doc| controller::execute(doc, command));
                        match result {
                            Some(Ok(())) => {
                                self.dialog = None;
                                self.refresh(id);
                            }
                            Some(Err(e)) => dialog.set_error(e.to_string()),
                            None => self.dialog = None,
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::NestedEdit(pointer, change) => {
                let Some(id) = self.active else {
                    return Task::none();
                };
                if !self.editable(id) {
                    self.not_editable(id);
                    return Task::none();
                }
                let Some((row, column)) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.grid.selected())
                else {
                    return Task::none();
                };
                self.leaf_edit = None;
                let result = self.change_nested_cell(id, row, column, |json| {
                    veta_core::values::edit_nested(json, &pointer, &change)
                });
                match result {
                    Ok(()) => self.refresh(id),
                    Err(e) => self.errors.push(e),
                }
            }
            Message::LeafInput(text) => {
                if let Some(edit) = &mut self.leaf_edit {
                    edit.text = text;
                    edit.error = None;
                }
            }
            Message::CommitLeaf => self.commit_leaf(),
            Message::ToggleNode(path) => {
                if !self.collapsed.remove(&path) {
                    self.collapsed.insert(path);
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
            Action::EditJson => {
                if let Some(id) = self.active {
                    self.open_json_editor(id);
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

    /// Applies the Value tab edit: the cell gets its JSON with the edited
    /// value replaced.
    fn commit_leaf(&mut self) {
        let Some(mut edit) = self.leaf_edit.take() else {
            return;
        };
        let id = edit.document;
        let text = (!edit.text.is_empty()).then_some(edit.text.clone());
        let (pointer, data_type, rename) =
            (edit.pointer.clone(), edit.data_type.clone(), edit.rename);
        let result = self.change_nested_cell(id, edit.row, edit.column, |json| {
            if rename {
                let key = veta_core::values::NestedEdit::RenameKey(text.unwrap_or_default());
                veta_core::values::edit_nested(json, &pointer, &key)
            } else {
                veta_core::values::set_nested(json, &pointer, text.as_deref(), &data_type)
            }
        });
        match result {
            Ok(()) => self.refresh(id),
            Err(e) => {
                edit.error = Some(e);
                self.leaf_edit = Some(edit);
            }
        }
    }

    /// Opens the selected nested cell in the multi-line JSON editor.
    pub(crate) fn open_json_editor(&mut self, id: DocumentId) {
        if !self.editable(id) {
            self.not_editable(id);
            return;
        }
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return;
        };
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let Some((row, column)) = tab.grid.selected() else {
            return;
        };
        let schema = doc.schema();
        let field = schema.field(column);
        if !field.data_type().is_nested() {
            return;
        }
        let Some(json) = tab
            .grid
            .cell_array(row, column, doc)
            .and_then(|a| veta_core::display::value_json(a.as_ref(), 0).ok())
        else {
            return;
        };
        self.edit = None;
        self.leaf_edit = None;
        self.dialog = Some(Dialog::Json {
            document: id,
            dialog: dialogs::JsonDialog::new(row, field.name().clone(), json.as_deref()),
        });
    }

    /// Replaces a nested cell with `change` applied to its JSON (`null` for
    /// a null cell).
    fn change_nested_cell(
        &mut self,
        id: DocumentId,
        row: usize,
        column: usize,
        change: impl FnOnce(&str) -> veta_core::Result<String>,
    ) -> Result<(), String> {
        let tab = self.tabs.iter().find(|t| t.id == id).ok_or("closed")?;
        let doc = self.workbook.get(id).ok_or("closed")?;
        let json = tab
            .grid
            .cell_array(row, column, doc)
            .and_then(|a| veta_core::display::value_json(a.as_ref(), 0).ok())
            .ok_or("the cell isn't loaded")?
            .unwrap_or_else(|| "null".into());
        let value = change(&json).map_err(|e| e.to_string())?;
        let name = doc.schema().field(column).name().clone();
        let doc = self.workbook.get_mut(id).ok_or("closed")?;
        controller::execute(
            doc,
            Command::SetCell {
                row,
                column: name,
                value: Some(value),
            },
        )
        .map_err(|e| e.to_string())
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

    /// The selected cell's value as a tree, for the Value tab.
    pub(crate) fn value_tree(
        &self,
        tab: &Tab,
        doc: &Document,
    ) -> Option<veta_core::display::ValueNode> {
        if self.side_tab != SideTab::Value || !self.show_side_pane {
            return None;
        }
        let (row, column) = tab.grid.selected()?;
        let array = tab.grid.cell_array(row, column, doc)?;
        let name = doc
            .schema_at(doc.evaluated_steps())?
            .field(column)
            .name()
            .clone();
        Some(veta_core::display::value_tree(array.as_ref(), 0, &name))
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
        })
    }

    /// Changes the list holding `item`.
    pub(crate) fn change_list(
        &mut self,
        id: DocumentId,
        item: &ListItem,
        change: &veta_core::values::ListEdit,
    ) -> Result<(), String> {
        let index = self
            .workbook
            .get(id)
            .and_then(|d| d.schema().index_of(&item.column).ok())
            .ok_or("the column no longer exists")?;
        let mut pointer: Vec<_> = item
            .list
            .iter()
            .cloned()
            .map(veta_core::display::PathPart::Field)
            .collect();
        // A struct field: only its value can be set.
        if let Some(field) = &item.field {
            let veta_core::values::ListEdit::Set {
                text, item_type, ..
            } = change
            else {
                return Err("struct fields can't be inserted or removed here".into());
            };
            pointer.push(veta_core::display::PathPart::Field(field.clone()));
            return self.change_nested_cell(id, item.row, index, |json| {
                veta_core::values::set_nested(json, &pointer, *text, item_type)
            });
        }
        self.change_nested_cell(id, item.row, index, |json| {
            veta_core::values::edit_list(json, &pointer, change)
        })
    }

    /// Inserts a null item above or below the selected item, or removes it,
    /// and selects the new item (or the one now in its place).
    pub(crate) fn insert_or_remove_item(
        &mut self,
        id: DocumentId,
        item: &ListItem,
        change: ItemChange,
    ) {
        use veta_core::values::ListEdit;
        if item.field.is_some() {
            return;
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
            ItemChange::Remove => return,
        };
        match self.change_list(id, item, &change) {
            Ok(()) => {
                self.refresh(id);
                self.select_item(id, Some(select));
            }
            Err(e) => self.errors.push(e),
        }
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
        let index = self
            .workbook
            .get(id)
            .and_then(|d| d.schema().index_of(column).ok())
            .ok_or("the column no longer exists")?;
        let pointer: Vec<_> = field
            .path
            .iter()
            .cloned()
            .map(veta_core::display::PathPart::Field)
            .collect();
        self.change_nested_cell(id, row, index, |json| {
            veta_core::values::set_nested(json, &pointer, text, &field.data_type)
        })
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
                crate::grid::OpenKind::List if initial.is_some() => usize::MAX,
                _ => 0,
            };
            self.select_item(id, Some(item));
        }
        let text = initial.unwrap_or_else(|| self.cell_text(id).unwrap_or_default());
        let item = self.selected_list_item(id);
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
        if self.cell_text(edit.document).as_deref() == Some(edit.text.as_str()) {
            return true;
        }
        let result = if let Some(item) = &edit.item {
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
