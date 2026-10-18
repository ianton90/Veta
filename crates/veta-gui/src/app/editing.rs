//! Editing cells and nested values (struct fields, list items, map
//! entries), and undo / redo.

use super::*;
use veta_core::display::PathPart;
use veta_core::values::node::NodeKind;

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
                // In a list or map, a value entered in the last item goes on
                // to a new one at the end (a map asks for its key first, then
                // its value); an empty new item ends. Struct fields go down.
                let edit = self.edit.clone();
                let item = edit.as_ref().and_then(|e| e.item.clone());
                let empty = edit.as_ref().is_some_and(|e| e.text.is_empty());
                let key = edit.as_ref().is_some_and(|e| e.key);
                if self.commit_edit()
                    && let Some(id) = self.active
                {
                    match item.map(|i| i.item) {
                        Some(item) if item.is_slot() && empty => {
                            self.grid_do(id, |g, d| {
                                g.select_path(&item.parent, d);
                            });
                        }
                        Some(item) if key && item.is_slot() => {
                            // The new entry: on to its value.
                            self.grid_do(id, |g, d| {
                                g.select_path(&item.path, d);
                            });
                            return self.start_edit(id, None);
                        }
                        Some(item) if key => {
                            self.grid_do(id, |g, d| {
                                g.select_path(&item.path, d);
                            });
                        }
                        Some(item) if item.parent_kind == NodeKind::Struct => {
                            self.grid_event(id, GridEvent::Navigate(Nav::Down));
                        }
                        Some(item) if item.index + 1 >= item.len && !item.fixed => {
                            self.grid_do(id, |g, d| g.select_slot(&item.parent, d));
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

    /// Opens the Add field dialog on the struct at `path` in the selected
    /// cell's column (`None`: the selected struct column, or the field of an
    /// expanded struct column the cell shows).
    pub(crate) fn open_add_field(
        &mut self,
        id: DocumentId,
        path: Option<Vec<PathPart>>,
    ) -> Task<Message> {
        let column = match path {
            Some(_) => self.selected_cell(id).map(|(_, c)| c),
            None => self.selected_column_names(id).into_iter().next(),
        };
        let path = path.unwrap_or_else(|| {
            self.tabs
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.grid.selected_field())
                .map(|(fields, _)| fields.iter().cloned().map(PathPart::Field).collect())
                .unwrap_or_default()
        });
        let is_struct = column.as_ref().is_some_and(|c| {
            self.workbook.get(id).is_some_and(|d| {
                d.schema().field_with_name(c).is_ok_and(|f| {
                    matches!(
                        veta_core::display::type_at(f.data_type(), &path),
                        Some(veta_core::arrow::datatypes::DataType::Struct(_))
                    )
                })
            })
        });
        match column {
            Some(column) if is_struct => {
                self.open_column_dialog(id, ColumnDialog::add_field(column, path))
            }
            _ => {
                self.errors.push("Select a struct first.".into());
                Task::none()
            }
        }
    }

    /// Removes the selected field of an open struct, at any depth, from its
    /// column's type.
    pub(crate) fn remove_field(&mut self, id: DocumentId) {
        let Some(item) = self.selected_cell_item(id) else {
            return;
        };
        let Some(name) = item.item.field().map(str::to_owned) else {
            return;
        };
        self.execute(
            id,
            Command::RemoveField {
                path: item.parent(),
                column: item.column,
                name,
            },
        );
    }

    /// Adds to the selected struct, list or map line (or nested cell): a
    /// field, an item at the end, or an entry at the end.
    pub(crate) fn add_inside(&mut self, id: DocumentId) -> Task<Message> {
        let (kind, path, cell) = match self.selected_cell_item(id) {
            Some(item) => match item.item.kind {
                Some(kind) if kind.is_container() => (kind, item.item.path.clone(), item.path()),
                _ => return Task::none(),
            },
            None => {
                let Some(kind) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.grid.selected_opens_items())
                else {
                    return Task::none();
                };
                (kind, Vec::new(), Vec::new())
            }
        };
        match kind {
            NodeKind::Struct if cell.is_empty() => self.open_add_field(id, None),
            NodeKind::Struct => self.open_add_field(id, Some(cell)),
            NodeKind::Map => {
                self.grid_do(id, |g, d| g.select_slot(&path, d));
                self.start_key_edit(id)
            }
            _ => {
                self.grid_do(id, |g, d| g.select_slot(&path, d));
                self.start_edit(id, None)
            }
        }
    }

    /// Row and column name of the selected cell in a document's grid.
    pub(crate) fn selected_cell(&self, id: DocumentId) -> Option<(usize, String)> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (row, column) = tab.grid.selected()?;
        let doc = self.workbook.get(id)?;
        Some((row, doc.schema().field(column).name().clone()))
    }

    /// Runs `f` on a document's grid.
    pub(crate) fn grid_do(&mut self, id: DocumentId, f: impl FnOnce(&mut GridView, &Document)) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            f(&mut tab.grid, doc);
        }
    }

    /// The selected line of an open cell, if one is.
    pub(crate) fn selected_cell_item(&self, id: DocumentId) -> Option<CellItem> {
        let (row, column) = self.selected_cell(id)?;
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let item = tab.grid.selected_item()?;
        let cell = tab
            .grid
            .selected_field()?
            .0
            .iter()
            .cloned()
            .map(PathPart::Field)
            .collect();
        Some(CellItem {
            row,
            column,
            cell,
            item,
        })
    }

    /// Sets the value of line `item` (a slot of a list: a new item at the
    /// end), from text; `None` sets null.
    pub(crate) fn set_item_value(
        &mut self,
        id: DocumentId,
        item: &CellItem,
        text: Option<&str>,
    ) -> Result<(), String> {
        let command = Command::SetValue {
            row: item.row,
            column: item.column.clone(),
            path: item.path(),
            value: text.map(str::to_owned),
        };
        self.nested_command(id, command)
    }

    /// Renames the key of map entry `item`, or adds an entry with that key
    /// when `item` is the slot for a new one.
    fn change_map_key(&mut self, id: DocumentId, item: &CellItem, key: &str) -> Result<(), String> {
        let (row, column, path, index) = (
            item.row,
            item.column.clone(),
            item.parent(),
            item.item.index,
        );
        let key = key.to_owned();
        let command = if item.item.is_slot() {
            Command::InsertItem {
                row,
                column,
                path,
                index,
                key: Some(key),
            }
        } else {
            Command::SetKey {
                row,
                column,
                path,
                index,
                key,
            }
        };
        self.nested_command(id, command)
    }

    /// Starts editing the key of the selected map entry (or the new key, on
    /// the slot).
    pub(crate) fn start_key_edit(&mut self, id: DocumentId) -> Task<Message> {
        let Some(item) = self.selected_cell_item(id) else {
            return Task::none();
        };
        let Some(key) = item.item.key().map(|k| k.unwrap_or_default().to_owned()) else {
            return Task::none();
        };
        self.edit = Some(Edit {
            document: id,
            row: item.row,
            column: item.column.clone(),
            field: None,
            text: key,
            item: Some(item),
            key: true,
            error: None,
        });
        Task::batch([
            iced::widget::operation::focus(EDITOR_ID),
            iced::widget::operation::move_cursor_to_end(EDITOR_ID),
        ])
    }

    /// Inserts a null item above or below the selected list item, or removes
    /// the selected item or entry, and selects the new item (or the one now
    /// in its place). Inserting into a map asks for the new entry's key
    /// there.
    pub(crate) fn insert_or_remove_item(
        &mut self,
        id: DocumentId,
        item: &CellItem,
        change: ItemChange,
    ) -> Task<Message> {
        let it = &item.item;
        if it.parent_kind == NodeKind::Struct {
            return Task::none();
        }
        if it.fixed {
            self.errors
                .push("Items of a fixed-size list can't be added or removed.".into());
            return Task::none();
        }
        // A map entry needs its key first: the slot for it opens there.
        if it.parent_kind == NodeKind::Map && change != ItemChange::Remove {
            let at = match change {
                ItemChange::InsertAbove => it.index,
                _ => it.index + 1,
            }
            .min(it.len);
            let parent = it.parent.clone();
            self.grid_do(id, |g, d| g.select_slot_at(&parent, Some(at), d));
            return self.start_key_edit(id);
        }
        let part = |i: usize| match it.parent_kind {
            NodeKind::Map => PathPart::Entry(i),
            _ => PathPart::Index(i),
        };
        let (row, column, path) = (item.row, item.column.clone(), item.parent());
        let (command, select) = match change {
            ItemChange::InsertAbove | ItemChange::InsertBelow => {
                let at = match change {
                    ItemChange::InsertAbove => it.index,
                    _ => it.index + 1,
                }
                .min(it.len);
                let command = Command::InsertItem {
                    row,
                    column,
                    path,
                    index: at,
                    key: None,
                };
                (command, Some(at))
            }
            ItemChange::Remove if !it.is_slot() => {
                let command = Command::RemoveItem {
                    row,
                    column,
                    path,
                    index: it.index,
                };
                let left = it.len - 1;
                (command, (left > 0).then(|| it.index.min(left - 1)))
            }
            ItemChange::Remove => return Task::none(),
        };
        match self.nested_command(id, command) {
            Ok(()) => {
                self.refresh(id);
                let target = match select {
                    Some(i) => [it.parent.clone(), vec![part(i)]].concat(),
                    None => it.parent.clone(),
                };
                self.grid_do(id, |g, d| {
                    g.select_path(&target, d);
                });
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

    /// Starts editing the selected cell or line in the formula bar.
    ///
    /// Structs, lists and maps aren't edited as text: Enter goes into them
    /// (to their first line, editing it if it is a plain value), and typing
    /// adds an item at the end of a list or map (a map entry starts with
    /// its key).
    pub(crate) fn start_edit(&mut self, id: DocumentId, initial: Option<String>) -> Task<Message> {
        let Some((row, column)) = self.selected_cell(id) else {
            return Task::none();
        };
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return Task::none();
        };
        // A nested cell: into its lines.
        if let Some(kind) = tab.grid.selected_opens_items()
            && tab.grid.selected_item().is_none()
            && !tab.grid.selected_is_line()
        {
            match (kind, &initial) {
                (NodeKind::List | NodeKind::Map, Some(_)) => {
                    self.grid_do(id, |g, d| g.select_slot(&[], d));
                }
                _ => self.grid_do(id, |g, d| g.select_item(Some(0), d)),
            }
        }
        let item = self.selected_cell_item(id);
        let Some(item) = item else {
            if self
                .tabs
                .iter()
                .find(|t| t.id == id)
                .is_some_and(|t| t.grid.selected_is_container())
            {
                return Task::none();
            }
            // A plain cell, or a field of an expanded struct column.
            let text = initial.unwrap_or_else(|| self.cell_text(id).unwrap_or_default());
            let field = self.selected_struct_field(id);
            return self.open_editor(Edit {
                document: id,
                row,
                column,
                field,
                item: None,
                key: false,
                text,
                error: None,
            });
        };
        let it = item.item.clone();
        if it.is_slot() && it.parent_kind == NodeKind::Map {
            let task = self.start_key_edit(id);
            if let (Some(edit), Some(text)) = (&mut self.edit, initial) {
                edit.text = text;
            }
            return task;
        }
        if it.is_slot() && NodeKind::of(&it.data_type).is_container() {
            // A new struct, list or map item: add it and go into it.
            let command = Command::InsertItem {
                row: item.row,
                column: item.column.clone(),
                path: item.parent(),
                index: it.index,
                key: None,
            };
            if let Err(e) = self.nested_command(id, command) {
                self.errors.push(e);
                return Task::none();
            }
            self.refresh(id);
            self.grid_do(id, |g, d| {
                g.select_path(&it.path, d);
            });
            return self.enter_container(id, &it.path, initial);
        }
        if it.is_container() {
            return self.enter_container(id, &it.path, initial);
        }
        let text = match initial {
            Some(text) => text,
            None if it.is_slot() => String::new(),
            None => self.cell_text(id).unwrap_or_default(),
        };
        self.open_editor(Edit {
            document: id,
            row,
            column,
            field: None,
            item: Some(item),
            key: false,
            text,
            error: None,
        })
    }

    /// Opens the selected struct, list or map line at `path` and goes to its
    /// first line; typing (`initial`) goes on as [`Self::start_edit`] there
    /// (at the end of a list or map).
    fn enter_container(
        &mut self,
        id: DocumentId,
        path: &[PathPart],
        initial: Option<String>,
    ) -> Task<Message> {
        let kind = self
            .selected_cell_item(id)
            .and_then(|i| i.item.kind)
            .unwrap_or(NodeKind::Struct);
        self.grid_do(id, |g, d| {
            g.open_path(path, d);
            match (kind, &initial) {
                (NodeKind::List | NodeKind::Map, Some(_)) => g.select_slot(path, d),
                _ => g.select_first_child(d),
            }
        });
        match initial {
            Some(_) => self.start_edit(id, initial),
            None => iced::widget::operation::focus(GRID_ID),
        }
    }

    /// Shows `edit` in the formula bar and the cell editor.
    fn open_editor(&mut self, edit: Edit) -> Task<Message> {
        self.edit = Some(edit);
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
                item.item.key().map(Option::unwrap_or_default) == Some(edit.text.as_str())
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
            self.set_item_value(edit.document, item, text)
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
                self.flash = Some((std::time::Instant::now(), 1.0));
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
