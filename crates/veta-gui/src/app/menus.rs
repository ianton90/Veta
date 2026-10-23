//! The right-click menu.

use super::*;

/// An open right-click menu.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContextMenu {
    pub(crate) document: DocumentId,
    pub(crate) target: MenuTarget,
    pub(crate) position: iced::Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuItem {
    Cut,
    Copy,
    Paste,
    EditCell,
    ClearCell,
    InsertRowsAbove,
    InsertRowsBelow,
    /// Appends a row (blank area menu).
    AddRow,
    /// Adds a column at the end (blank area menu).
    AddColumn,
    KeepRows,
    DeleteRows,
    InsertColumnLeft,
    InsertColumnRight,
    RenameColumn,
    RemoveColumns,
    KeepColumns,
    MoveColumnLeft,
    MoveColumnRight,
    FilterColumn,
    ChangeType,
    ReplaceValues,
    FillDown,
    FillUp,
    FormatText,
    DuplicateColumn,
    InsertItem,
    InsertItemBelow,
    AddField,
    RemoveField,
    DeleteItem,
    /// Edit the key of a map entry.
    EditKey,
    /// Add to the selected struct, list or map: a field, or an item or
    /// entry at the end.
    AddInside,
    /// Expand a struct column and the structs inside it.
    SortAscending,
    SortDescending,
    /// Filter to rows whose value in this column equals the selected cell's.
    KeepValue,
    /// Filter out rows whose value in this column equals the selected cell's.
    ExcludeValue,
}

impl MenuItem {
    /// The ribbon action this item does, when it does exactly that.
    fn action(self) -> Option<Action> {
        Some(match self {
            MenuItem::Cut => Action::Cut,
            MenuItem::Copy => Action::Copy,
            MenuItem::Paste => Action::Paste,
            MenuItem::RenameColumn => Action::RenameColumn,
            MenuItem::MoveColumnLeft => Action::MoveColumnLeft,
            MenuItem::MoveColumnRight => Action::MoveColumnRight,
            MenuItem::FilterColumn => Action::FilterRows,
            MenuItem::ChangeType => Action::DataType,
            MenuItem::ReplaceValues => Action::ReplaceValues,
            MenuItem::FillDown => Action::FillDown,
            MenuItem::FillUp => Action::FillUp,
            MenuItem::FormatText => Action::FormatText,
            MenuItem::DuplicateColumn => Action::DuplicateColumn,
            MenuItem::SortAscending => Action::SortAscending,
            MenuItem::SortDescending => Action::SortDescending,
            _ => return None,
        })
    }
}

impl App {
    /// Right-click menu messages.
    pub(crate) fn menus_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Menu(item) => {
                let Some(menu) = self.menu.take() else {
                    return Task::none();
                };
                // Items that are ribbon actions go the same way (on the menu's
                // document, which is the active one).
                if let Some(action) = item.action() {
                    self.active = Some(menu.document);
                    return self.perform(action);
                }
                let id = menu.document;
                let views_only = matches!(item, MenuItem::KeepValue | MenuItem::ExcludeValue);
                if !views_only && !self.editable(id) {
                    self.not_editable(id);
                    return Task::none();
                }
                match item {
                    MenuItem::KeepRows => self.keep_rows(id),
                    MenuItem::EditCell => return self.start_edit(id, None),
                    MenuItem::AddField => {
                        // A struct field's menu adds to the struct holding it.
                        let path = self.selected_cell_item(id).map(|i| i.parent());
                        return self.open_add_field(id, path);
                    }
                    MenuItem::RemoveField => self.remove_field(id),
                    MenuItem::EditKey => return self.start_key_edit(id),
                    MenuItem::AddInside => return self.add_inside(id),
                    MenuItem::InsertItem | MenuItem::InsertItemBelow | MenuItem::DeleteItem => {
                        if let Some(list_item) = self.selected_cell_item(id) {
                            let change = match item {
                                MenuItem::InsertItem => ItemChange::InsertAbove,
                                MenuItem::InsertItemBelow => ItemChange::InsertBelow,
                                _ => ItemChange::Remove,
                            };
                            return self.insert_or_remove_item(id, &list_item, change);
                        }
                    }
                    MenuItem::ClearCell => {
                        return self.update(Message::Grid(id, GridEvent::ClearCell));
                    }
                    MenuItem::InsertRowsAbove => self.insert_rows(id, false),
                    MenuItem::InsertRowsBelow => self.insert_rows(id, true),
                    MenuItem::AddRow => self.append_row(id),
                    MenuItem::AddColumn => {
                        let at = self.workbook.get(id).map_or(0, Document::num_columns);
                        return self.open_column_dialog(id, ColumnDialog::add(at), None);
                    }
                    MenuItem::DeleteRows => self.delete_rows(id),
                    MenuItem::InsertColumnLeft | MenuItem::InsertColumnRight => {
                        let columns = self.selected_columns(id);
                        let at = match (item, columns) {
                            (MenuItem::InsertColumnLeft, Some(c)) => c.start,
                            (_, Some(c)) => c.end,
                            (_, None) => 0,
                        };
                        return self.open_column_dialog(id, ColumnDialog::add(at), None);
                    }
                    MenuItem::RemoveColumns => self.keep_or_remove_columns(id, false, true),
                    MenuItem::KeepColumns => self.keep_or_remove_columns(id, true, true),
                    MenuItem::KeepValue | MenuItem::ExcludeValue => {
                        self.quick_filter(id, item == MenuItem::KeepValue);
                    }
                    _ => unreachable!("ribbon actions are performed above"),
                }
            }
            Message::CloseMenu => self.menu = None,
            _ => unreachable!("not a menus message"),
        }
        Task::none()
    }
}
