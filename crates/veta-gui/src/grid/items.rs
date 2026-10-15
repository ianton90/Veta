//! Open list, map and struct cells: their item lines and item selection.

use super::*;

/// What kind of value an open cell lists the items of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ItemsKind {
    /// Not a list, map or struct: nothing listed.
    #[default]
    None,
    /// An editable list (see [`Column::list`]).
    List,
    /// A map: entries editable (see [`Column::map`]).
    Map,
    /// Fixed-size lists: shown only.
    ReadOnly,
    /// Struct fields, editable.
    Struct,
}

/// The items listed in an open list, map or struct cell.
#[derive(Debug, Clone, Default)]
pub(super) struct Items {
    pub(super) items: Vec<veta_core::display::ListItem>,
    pub(super) more: usize,
    pub(super) kind: ItemsKind,
}

impl Items {
    /// Screen lines the items take, with the slot for a new item when the
    /// list is empty or `appending`.
    pub(super) fn lines(&self, appending: bool) -> usize {
        self.items.len() + usize::from(self.more > 0 || self.has_slot(appending))
    }

    /// Whether the slot for a new item is shown.
    pub(super) fn has_slot(&self, appending: bool) -> bool {
        matches!(self.kind, ItemsKind::List | ItemsKind::Map)
            && self.more == 0
            && (self.items.is_empty() || appending)
    }

    /// Items that can be selected: the items and the slot, if shown.
    fn selectable(&self, appending: bool) -> usize {
        match self.kind {
            ItemsKind::List | ItemsKind::Map => {
                self.items.len() + usize::from(self.has_slot(appending))
            }
            ItemsKind::Struct => self.items.len(),
            ItemsKind::None | ItemsKind::ReadOnly => 0,
        }
    }
}

impl GridView {
    /// The selected list item, struct field or map entry: its index, the
    /// number of items and the item (or value) type. The list is at
    /// [`Self::selected_field`] in the cell. The index equals the number of
    /// items for the slot of a new item.
    pub fn selected_item(&self) -> Option<SelectedItem> {
        let (row, column) = self.selected?;
        let index = self.item?;
        let items = self.items(row, column)?;
        let len = items.items.len();
        let data_type = &self.columns.get(column)?.data_type;
        match data_type {
            DataType::List(f) | DataType::LargeList(f) => Some(SelectedItem {
                index,
                len,
                data_type: f.data_type().clone(),
                field: None,
                key: None,
            }),
            DataType::Struct(fields) => {
                let field = fields.get(index)?;
                Some(SelectedItem {
                    index,
                    len,
                    data_type: field.data_type().clone(),
                    field: Some(field.name().clone()),
                    key: None,
                })
            }
            _ => {
                let (_, value) = map_types(data_type)?;
                Some(SelectedItem {
                    index,
                    len,
                    data_type: value.clone(),
                    field: None,
                    key: Some(items.items.get(index).map(|i| i.label.clone())),
                })
            }
        }
    }

    /// Whether the selected cell is a list or struct whose items (fields)
    /// can be edited in place.
    pub fn selected_opens_items(&self) -> Option<OpenKind> {
        let column = self.columns.get(self.selected?.1)?;
        if column.list {
            Some(OpenKind::List)
        } else if column.map {
            Some(OpenKind::Map)
        } else if column.is_struct {
            Some(OpenKind::Struct)
        } else {
            None
        }
    }

    /// Opens the selected cell's row and selects item `item` of it (`None`:
    /// the cell itself). An item past the end selects the new-item slot.
    pub fn select_item(&mut self, item: Option<usize>, document: &Document) {
        let Some((row, column)) = self.selected else {
            return;
        };
        if item.is_some() && !self.open_rows.contains(&row) {
            self.open_rows.insert(row);
            self.load(document, true);
        }
        let len = self.items(row, column).map(|i| i.items.len());
        self.item = match (item, len) {
            (Some(i), Some(len)) => Some(i.min(len)),
            _ => None,
        };
        self.rows = None;
        self.cols = None;
        self.reveal(row, column);
    }

    /// Whether the slot for a new item of cell (`row`, `column`) is selected.
    pub(super) fn appending(&self, row: usize, column: usize) -> bool {
        self.selected == Some((row, column))
            && self.item.is_some()
            && self.item == self.items(row, column).map(|i| i.items.len())
    }

    /// The listed items of a cell of an open row.
    pub(super) fn items(&self, row: usize, column: usize) -> Option<&Items> {
        self.details.get(&row)?.get(column)
    }

    /// Items that can be selected in cell (`row`, `column`): 0 unless it is
    /// an editable list in an open row.
    pub(super) fn selectable_items(&self, row: usize, column: usize) -> usize {
        self.items(row, column)
            .map_or(0, |i| i.selectable(self.appending(row, column)))
    }

    /// Up and down through the items of open list cells. Returns whether
    /// it handled `nav`.
    pub(super) fn navigate_items(&mut self, nav: Nav) -> bool {
        let Some((row, column)) = self.selected else {
            return false;
        };
        let count = self.selectable_items(row, column);
        match (nav, self.item) {
            (Nav::Down, None) if count > 0 => self.item = Some(0),
            (Nav::Down, Some(i)) if i + 1 < count => self.item = Some(i + 1),
            (Nav::Down, Some(_)) => {
                self.item = None;
                return false;
            }
            (Nav::Up, Some(0)) => self.item = None,
            (Nav::Up, Some(i)) => self.item = Some(i - 1),
            (Nav::Up, None) if row > 0 && self.selectable_items(row - 1, column) > 0 => {
                let items = self.items(row - 1, column).map_or(0, |i| i.items.len());
                self.selected = Some((row - 1, column));
                self.item = Some(items.saturating_sub(1));
            }
            _ => return false,
        }
        self.reveal(row, column);
        true
    }

    /// Whether a loaded list or map cell has items to open.
    pub(super) fn has_items(&self, row: usize, column: usize) -> bool {
        // Editable lists and maps open even when empty, to add items.
        self.columns
            .get(column)
            .is_some_and(|c| (c.list || c.map || c.is_struct) && self.value(row, column).is_some())
            || self.columns.get(column).is_some_and(|c| c.items)
                && matches!(self.value(row, column), Some(Some(v)) if v != "[]" && v != "{}")
    }
}
