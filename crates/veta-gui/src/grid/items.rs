//! Open cells: nested values shown as trees of lines (struct fields, list
//! items and map entries, each opened on its own), and selecting lines.
//!
//! Opening a row lists the children of its nested cells; a child that is a
//! struct, list or map opens in turn, indented one level more. Lines are
//! built from the loaded rows with [`veta_core::values::node::Node`].

use super::*;
use veta_core::display::{PathPart, type_at};
use veta_core::values::node::{Node, NodeKind};

/// One screen line of an open cell.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Line {
    /// From the cell down to the value; for a slot or "more" line, the
    /// path of the list or map it belongs to.
    pub(super) path: Vec<PathPart>,
    /// 1 for the cell's children, 2 for theirs, …
    pub(super) depth: usize,
    /// Field name, index or key.
    pub(super) label: String,
    /// As shown: plain values as text, lists and maps as their size; `None`
    /// for null.
    pub(super) text: Option<String>,
    /// Space is kept for an open/close arrow before the label (the parent
    /// has nested children), so labels of siblings line up.
    pub(super) gutter: bool,
    pub(super) what: What,
}

/// What a line shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum What {
    /// A value: its kind, whether it is open, and its place in its parent.
    Node {
        kind: NodeKind,
        open: bool,
        at: Place,
    },
    /// Where a new item or entry is typed, after the last one of an open
    /// list or map.
    Slot(Place),
    /// Children not listed.
    More(usize),
}

/// Where a value sits: the kind of its parent, its position and the
/// parent's number of children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Place {
    pub(super) parent: NodeKind,
    pub(super) index: usize,
    pub(super) len: usize,
    /// The parent is a fixed-size list: items can't be added or removed.
    pub(super) fixed: bool,
}

/// The lines of one open cell.
#[derive(Debug, Clone, Default)]
pub(super) struct CellTree {
    pub(super) lines: Vec<Line>,
}

impl CellTree {
    /// The lines of an open cell holding `root`: its children, and the
    /// children of those for which `open` holds. Lists and maps get a slot
    /// line when empty, or where `slot` says: their path, and the position
    /// of the slot (`None`: after the last child).
    pub(super) fn of(
        root: &Node,
        open: &dyn Fn(&[PathPart]) -> bool,
        slot: Option<(&[PathPart], Option<usize>)>,
    ) -> Self {
        let mut lines = Vec::new();
        push_children(root, &mut Vec::new(), 1, open, slot, &mut lines);
        Self { lines }
    }
}

fn push_children(
    node: &Node,
    path: &mut Vec<PathPart>,
    depth: usize,
    open: &dyn Fn(&[PathPart]) -> bool,
    slot: Option<(&[PathPart], Option<usize>)>,
    lines: &mut Vec<Line>,
) {
    let kind = node.kind();
    let len = node.len();
    let fixed = node.is_fixed_size();
    let gutter = match node.data_type() {
        DataType::Struct(fields) => fields
            .iter()
            .any(|f| NodeKind::of(f.data_type()).is_container()),
        t => type_at(t, &[PathPart::Index(0)])
            .or_else(|| type_at(t, &[PathPart::Entry(0)]))
            .is_some_and(|t| NodeKind::of(t).is_container()),
    };
    let can_add = matches!(kind, NodeKind::List | NodeKind::Map) && !fixed;
    // Where this list or map shows a slot, if it does.
    let slot_at = match slot {
        Some((p, at)) if can_add && p == path.as_slice() => Some(at.unwrap_or(len).min(len)),
        _ if can_add && len == 0 => Some(0),
        _ => None,
    };
    let slot_line = |path: &[PathPart], index| Line {
        path: path.to_vec(),
        depth,
        label: String::new(),
        text: None,
        gutter,
        what: What::Slot(Place {
            parent: kind,
            index,
            len,
            fixed,
        }),
    };
    for (index, child) in node.children().take(MAX_ITEMS).enumerate() {
        if slot_at == Some(index) {
            lines.push(slot_line(path, index));
        }
        path.push(child.part);
        let child_kind = child.node.kind();
        let is_open = child_kind.is_container() && open(path);
        lines.push(Line {
            path: path.clone(),
            depth,
            label: child.label,
            text: child.node.summary(),
            gutter,
            what: What::Node {
                kind: child_kind,
                open: is_open,
                at: Place {
                    parent: kind,
                    index,
                    len,
                    fixed,
                },
            },
        });
        if is_open {
            push_children(&child.node, path, depth + 1, open, slot, lines);
        }
        path.pop();
    }
    if len > MAX_ITEMS {
        lines.push(Line {
            path: path.clone(),
            depth,
            label: String::new(),
            text: None,
            gutter,
            what: What::More(len - MAX_ITEMS),
        });
    } else if slot_at == Some(len) {
        lines.push(slot_line(path, len));
    }
}

/// The selected line of an open cell (see [`GridView::selected_item`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedItem {
    /// From the cell down to the value; for the slot, to the new item.
    pub path: Vec<PathPart>,
    /// The struct, list or map holding it.
    pub parent: Vec<PathPart>,
    pub parent_kind: NodeKind,
    /// Position in the parent (its number of children for the slot).
    pub index: usize,
    /// Number of children of the parent.
    pub len: usize,
    /// The value's kind; `None` for the slot.
    pub kind: Option<NodeKind>,
    /// The value's type (a map entry's value type).
    pub data_type: DataType,
    /// Field name, index or key (empty for the slot).
    pub label: String,
    /// The path as text, e.g. `[0].options["color"]`.
    pub display: String,
    /// The parent is a fixed-size list.
    pub fixed: bool,
}

impl SelectedItem {
    /// The slot for a new item or entry.
    pub fn is_slot(&self) -> bool {
        self.kind.is_none()
    }

    /// A struct, list or map: it opens instead of being edited.
    pub fn is_container(&self) -> bool {
        self.kind.is_some_and(NodeKind::is_container)
    }

    /// The field name, for a struct field.
    pub fn field(&self) -> Option<&str> {
        (self.parent_kind == NodeKind::Struct).then_some(self.label.as_str())
    }

    /// For a map entry, its key (`None` inside for the slot).
    pub fn key(&self) -> Option<Option<&str>> {
        (self.parent_kind == NodeKind::Map)
            .then(|| (!self.is_slot()).then_some(self.label.as_str()))
    }
}

impl GridView {
    /// Builds the lines of the open cells from the loaded rows.
    pub(super) fn build_trees(&mut self) {
        self.trees.clear();
        let Some(batch) = &self.batch else {
            return;
        };
        let start = self.window_start;
        let end = start + batch.num_rows();
        let mut trees = std::collections::HashMap::new();
        for &row in self.open_rows.range(start..end) {
            let cells = batch
                .columns()
                .iter()
                .zip(&self.columns)
                .enumerate()
                .map(|(c, (array, column))| {
                    if !NodeKind::of(array.data_type()).is_container() {
                        return CellTree::default();
                    }
                    let root = Node::new(array.clone(), row - start);
                    let open = |path: &[PathPart]| {
                        self.open_nodes
                            .contains(&(row, column.key.clone(), path.to_vec()))
                    };
                    let slot = self
                        .slot
                        .as_ref()
                        .filter(|(r, sc, _, _)| (*r, *sc) == (row, c))
                        .map(|(_, _, p, at)| (p.as_slice(), *at));
                    CellTree::of(&root, &open, slot)
                })
                .collect();
            trees.insert(row, cells);
        }
        self.trees = trees;
    }

    /// Rebuilds the lines after opening, closing or a slot change, keeping
    /// the same line selected.
    fn rebuild_trees(&mut self) {
        let kept = self.selected_line().cloned();
        self.build_trees();
        if let (Some(kept), Some((row, column))) = (kept, self.selected) {
            self.item = self
                .tree(row, column)
                .and_then(|t| t.lines.iter().position(|l| same_line(l, &kept)));
        }
    }

    /// The selected line of an open cell.
    pub(super) fn selected_line(&self) -> Option<&Line> {
        let (row, column) = self.selected?;
        self.tree(row, column)?.lines.get(self.item?)
    }

    /// The selected line: its path, place, kind and type. `None` unless a
    /// line of an open cell is selected (or it is a "more" line).
    pub fn selected_item(&self) -> Option<SelectedItem> {
        let (row, column) = self.selected?;
        let line = self.selected_line()?;
        let (kind, at) = match &line.what {
            What::Node { kind, at, .. } => (Some(*kind), *at),
            What::Slot(at) => (None, *at),
            What::More(_) => return None,
        };
        let (parent, path) = match kind {
            Some(_) => (line.path[..line.path.len() - 1].to_vec(), line.path.clone()),
            None => {
                let mut path = line.path.clone();
                path.push(match at.parent {
                    NodeKind::Map => PathPart::Entry(at.index),
                    _ => PathPart::Index(at.index),
                });
                (line.path.clone(), path)
            }
        };
        let column_type = &self.columns.get(column)?.data_type;
        let data_type = match kind {
            Some(_) => type_at(column_type, &path)?.clone(),
            None => {
                let mut first = parent.clone();
                first.push(match at.parent {
                    NodeKind::Map => PathPart::Entry(0),
                    _ => PathPart::Index(0),
                });
                type_at(column_type, &first)?.clone()
            }
        };
        let display = match (kind, at.parent) {
            (None, NodeKind::Map) => {
                format!("{}[new key]", self.display_path(row, column, &parent))
            }
            _ => self.display_path(row, column, &path),
        };
        Some(SelectedItem {
            display,
            path,
            parent,
            parent_kind: at.parent,
            index: at.index,
            len: at.len,
            kind,
            data_type,
            label: line.label.clone(),
            fixed: at.fixed,
        })
    }

    /// `path` in cell (`row`, `column`) as text: `.field`, `[index]`,
    /// `["key"]` (keys as shown on their lines).
    fn display_path(&self, row: usize, column: usize, path: &[PathPart]) -> String {
        let lines = self.tree(row, column).map_or(&[][..], |t| &t.lines[..]);
        let mut out = String::new();
        for (i, part) in path.iter().enumerate() {
            match part {
                PathPart::Field(name) => out.push_str(&format!(".{name}")),
                PathPart::Index(n) => out.push_str(&format!("[{n}]")),
                PathPart::Entry(n) => {
                    let key = lines
                        .iter()
                        .find(|l| l.path == path[..=i] && matches!(l.what, What::Node { .. }))
                        .map_or_else(|| format!("#{n}"), |l| format!("{:?}", l.label));
                    out.push_str(&format!("[{key}]"));
                }
            }
        }
        out
    }

    /// The kind of the selected cell's column if it is a struct, list or
    /// map (its cells open into lines).
    pub fn selected_opens_items(&self) -> Option<NodeKind> {
        let column = self.columns.get(self.selected?.1)?;
        Some(NodeKind::of(&column.data_type)).filter(|k| k.is_container())
    }

    /// The selected cell is a struct, list or map, or a line of one that
    /// is: it opens rather than being edited as text.
    pub fn selected_is_container(&self) -> bool {
        match self.item {
            Some(_) => self.selected_line().is_some_and(|l| match l.what {
                What::Node { kind, .. } => kind.is_container(),
                What::More(_) => true,
                What::Slot(_) => false,
            }),
            None => self.selected_opens_items().is_some(),
        }
    }

    /// Whether a line of an open cell is selected (not the cell itself).
    pub fn selected_is_line(&self) -> bool {
        self.item.is_some()
    }

    /// Selects the line after the selected one: the first child of an open
    /// struct, list or map (or its slot).
    pub fn select_first_child(&mut self, document: &Document) {
        if let Some(i) = self.item {
            self.select_item(Some(i + 1), document);
        }
    }

    /// Opens the selected cell's row and selects line `item` of it (`None`:
    /// the cell itself; past the end: the last line).
    pub fn select_item(&mut self, item: Option<usize>, document: &Document) {
        let Some((row, column)) = self.selected else {
            return;
        };
        if item.is_some() && !self.open_rows.contains(&row) {
            self.open_rows.insert(row);
            self.load(document, true);
        }
        let len = self.tree(row, column).map(|t| t.lines.len());
        self.item = match (item, len) {
            (Some(i), Some(len)) if len > 0 => Some(i.min(len - 1)),
            _ => None,
        };
        self.after_select();
        self.rows = None;
        self.cols = None;
        self.reveal(row, column);
    }

    /// Opens the selected cell's row and selects the line of the value at
    /// `path` (empty: the cell itself). Returns whether it is there.
    pub fn select_path(&mut self, path: &[PathPart], document: &Document) -> bool {
        if path.is_empty() {
            self.select_item(None, document);
            return true;
        }
        self.select_item(Some(0), document);
        let Some((row, column)) = self.selected else {
            return false;
        };
        let found = self.tree(row, column).and_then(|t| {
            t.lines
                .iter()
                .position(|l| l.path == path && matches!(l.what, What::Node { .. }))
        });
        if found.is_some() {
            self.item = found;
        }
        self.after_select();
        self.reveal(row, column);
        found.is_some()
    }

    /// Opens the value at `path` in the selected cell (and its row).
    pub fn open_path(&mut self, path: &[PathPart], document: &Document) {
        let Some((row, column)) = self.selected else {
            return;
        };
        if !self.open_rows.contains(&row) {
            self.open_rows.insert(row);
            self.load(document, true);
        }
        if let Some(key) = self.columns.get(column).map(|c| c.key.clone())
            && !path.is_empty()
            && self.open_nodes.insert((row, key, path.to_vec()))
        {
            self.rebuild_trees();
        }
    }

    /// Shows the slot after the last item of the list or map at `path` in
    /// the selected cell (opening it) and selects it.
    pub fn select_slot(&mut self, path: &[PathPart], document: &Document) {
        self.select_slot_at(path, None, document);
    }

    /// Shows the slot for a new item of the list or map at `path` in the
    /// selected cell before child `at` (`None`: after the last one), opening
    /// it, and selects it.
    pub fn select_slot_at(&mut self, path: &[PathPart], at: Option<usize>, document: &Document) {
        let Some((row, column)) = self.selected else {
            return;
        };
        self.open_path(path, document);
        self.slot = Some((row, column, path.to_vec(), at));
        self.build_trees();
        self.item = self.tree(row, column).and_then(|t| {
            t.lines
                .iter()
                .position(|l| l.path == path && matches!(l.what, What::Slot(_)))
        });
        // A fixed-size list has no slot: its first item instead.
        if self.item.is_none() {
            self.slot = None;
            self.item = self.tree(row, column).and_then(|t| {
                t.lines.iter().position(|l| {
                    l.path.len() == path.len() + 1
                        && l.path.starts_with(path)
                        && matches!(l.what, What::Node { .. })
                })
            });
        }
        self.rows = None;
        self.cols = None;
        self.reveal(row, column);
    }

    /// Opens or closes the selected line (a struct, list or map), or the
    /// selected cell's row when no line is selected.
    pub(super) fn toggle_selected(&mut self, document: &Document) {
        let Some((row, column)) = self.selected else {
            return;
        };
        if self.item.is_none() {
            if self.has_items(row, column) {
                self.toggle_row(row, document);
            }
            return;
        }
        let Some(line) = self.selected_line() else {
            return;
        };
        if !matches!(line.what, What::Node { kind, .. } if kind.is_container()) {
            return;
        }
        let Some(key) = self.columns.get(column).map(|c| c.key.clone()) else {
            return;
        };
        let node = (row, key, line.path.clone());
        if !self.open_nodes.remove(&node) {
            self.open_nodes.insert(node);
        }
        self.rebuild_trees();
        self.reveal(row, column);
    }

    /// Opens or closes the nested cells of `row`.
    pub(super) fn toggle_row(&mut self, row: usize, document: &Document) {
        if !self.open_rows.remove(&row) && row < self.num_rows {
            self.open_rows.insert(row);
        }
        if self.selected.is_some_and(|(r, _)| r == row) {
            self.item = None;
        }
        self.load(document, true);
        self.reveal(row, self.selected.map_or(0, |(_, c)| c));
    }

    /// Drops the slot once its line is no longer selected.
    pub(super) fn after_select(&mut self) {
        let on_slot = self
            .selected_line()
            .is_some_and(|l| matches!(l.what, What::Slot(_)));
        if self.slot.is_some() && !on_slot {
            self.slot = None;
            self.rebuild_trees();
        }
    }

    /// Closes every open row and value.
    pub(super) fn close_all(&mut self) {
        self.open_rows.clear();
        self.open_nodes.clear();
        self.slot = None;
    }

    /// The lines of a cell of an open row.
    pub(super) fn tree(&self, row: usize, column: usize) -> Option<&CellTree> {
        self.trees.get(&row)?.get(column)
    }

    /// Whether line `item` of cell (`row`, `column`) can be selected (all
    /// but "more" lines can).
    pub(super) fn is_selectable(&self, row: usize, column: usize, item: usize) -> bool {
        self.tree(row, column)
            .and_then(|t| t.lines.get(item))
            .is_some_and(|l| !matches!(l.what, What::More(_)))
    }

    /// Up and down through the lines of open cells. Returns whether it
    /// handled `nav`.
    pub(super) fn navigate_items(&mut self, nav: Nav) -> bool {
        let Some((row, column)) = self.selected else {
            return false;
        };
        let lines = |row| self.tree(row, column).map_or(0, |t| t.lines.len());
        let count = lines(row);
        let next =
            (self.item.map_or(0, |i| i + 1)..count).find(|&i| self.is_selectable(row, column, i));
        match (nav, self.item) {
            (Nav::Down, _) if next.is_some() => self.item = next,
            (Nav::Down, Some(_)) => {
                self.item = None;
                self.after_select();
                return false;
            }
            (Nav::Up, Some(i)) => {
                self.item = (0..i).rev().find(|&i| self.is_selectable(row, column, i));
            }
            (Nav::Up, None) if row > 0 && lines(row - 1) > 0 => {
                let last = (0..lines(row - 1))
                    .rev()
                    .find(|&i| self.is_selectable(row - 1, column, i));
                self.selected = Some((row - 1, column));
                self.item = last;
            }
            _ => return false,
        }
        self.after_select();
        self.reveal(row, column);
        true
    }

    /// Whether a loaded cell is a struct, list or map with lines to open.
    pub(super) fn has_items(&self, row: usize, column: usize) -> bool {
        // Lists and maps open even when empty or null, to add items.
        self.columns.get(column).is_some_and(|c| {
            NodeKind::of(&c.data_type).is_container() && self.value(row, column).is_some()
        })
    }
}

/// The same line, after the lines were built again.
fn same_line(a: &Line, b: &Line) -> bool {
    a.path == b.path
        && matches!(
            (&a.what, &b.what),
            (What::Node { .. }, What::Node { .. })
                | (What::Slot(_), What::Slot(_))
                | (What::More(_), What::More(_))
        )
}
