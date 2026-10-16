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
    SplitColumn,
    MergeColumns,
    DuplicateColumn,
    ExpandStruct,
    CombineStruct,
    CollectList,
    InsertItem,
    InsertItemBelow,
    AddField,
    RemoveField,
    DeleteItem,
    /// Edit the key of a map entry.
    EditKey,
    /// Add an entry at the end of a map.
    AddEntry,
    /// Expand a struct column and the structs inside it.
    ExpandAll,
    ExplodeList,
    SortAscending,
    SortDescending,
    /// Filter to rows whose value in this column equals the selected cell's.
    KeepValue,
    /// Filter out rows whose value in this column equals the selected cell's.
    ExcludeValue,
}

impl App {
    /// Right-click menu messages.
    pub(crate) fn menus_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Menu(item) => {
                let Some(menu) = self.menu.take() else {
                    return Task::none();
                };
                let id = menu.document;
                let views_only = matches!(
                    item,
                    MenuItem::KeepValue
                        | MenuItem::ExcludeValue
                        | MenuItem::FilterColumn
                        | MenuItem::Copy
                );
                if !views_only && !self.editable(id) {
                    self.not_editable(id);
                    return Task::none();
                }
                match item {
                    MenuItem::KeepRows => self.keep_rows(id),
                    MenuItem::EditCell => return self.start_edit(id, None),
                    MenuItem::AddField => return self.open_add_field(id),
                    MenuItem::RemoveField => self.remove_field(id),
                    MenuItem::EditKey => return self.start_key_edit(id),
                    MenuItem::AddEntry => return self.add_map_entry(id),
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
                        return self.open_column_dialog(id, ColumnDialog::add(at));
                    }
                    MenuItem::DeleteRows => self.delete_rows(id),
                    MenuItem::InsertColumnLeft | MenuItem::InsertColumnRight => {
                        let columns = self.selected_columns(id);
                        let at = match (item, columns) {
                            (MenuItem::InsertColumnLeft, Some(c)) => c.start,
                            (_, Some(c)) => c.end,
                            (_, None) => 0,
                        };
                        return self.open_column_dialog(id, ColumnDialog::add(at));
                    }
                    MenuItem::RenameColumn => return self.rename_column(id),
                    MenuItem::RemoveColumns => self.keep_or_remove_columns(id, false, true),
                    MenuItem::KeepColumns => self.keep_or_remove_columns(id, true, true),
                    MenuItem::FilterColumn => return self.open_filter_dialog(id),
                    MenuItem::ChangeType => self.open_cast_dialog(id, None),
                    MenuItem::ReplaceValues => return self.open_replace_dialog(id, None),
                    MenuItem::FillDown | MenuItem::FillUp => {
                        self.fill_selected(id, item == MenuItem::FillUp);
                    }
                    MenuItem::FormatText => self.open_format_dialog(id),
                    MenuItem::SplitColumn => return self.open_split_dialog(id, None),
                    MenuItem::MergeColumns => return self.open_merge_dialog(id, None),
                    MenuItem::CombineStruct => return self.open_combine_dialog(id, None),
                    MenuItem::CollectList => self.open_collect_dialog(id, None),
                    MenuItem::DuplicateColumn => self.duplicate_column(id),
                    MenuItem::Cut => return self.copy_selection(id, true),
                    MenuItem::Copy => return self.copy_selection(id, false),
                    MenuItem::Paste => return iced::clipboard::read().map(Message::Pasted),
                    MenuItem::ExpandStruct => self.nested_step(id, false, false),
                    MenuItem::ExpandAll => self.nested_step(id, false, true),
                    MenuItem::ExplodeList => self.nested_step(id, true, false),
                    MenuItem::SortAscending | MenuItem::SortDescending => {
                        self.sort_selected(id, item == MenuItem::SortDescending);
                    }
                    MenuItem::KeepValue | MenuItem::ExcludeValue => {
                        self.quick_filter(id, item == MenuItem::KeepValue);
                    }
                    MenuItem::MoveColumnLeft | MenuItem::MoveColumnRight => {
                        if let Some(c) = self.selected_columns(id) {
                            let to = if item == MenuItem::MoveColumnLeft {
                                c.start.checked_sub(1)
                            } else {
                                Some(c.start + 1)
                            };
                            if let Some(to) = to {
                                self.move_column(id, c.start, to);
                            }
                        }
                    }
                }
            }
            Message::CloseMenu => self.menu = None,
            _ => unreachable!("not a menus message"),
        }
        Task::none()
    }
}
