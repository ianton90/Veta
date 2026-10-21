//! Nested value commands: values, list items and map entries at any depth
//! inside a cell. Each becomes an edit of the whole cell, through its JSON.

use super::*;
use crate::error::invalid;
use arrow::datatypes::DataType;

use crate::display::{PathPart, map_key_type, type_at, value_json};
use crate::values::{ListEdit, MapEdit, edit_list, edit_map, fill_null_list, set_nested};

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<Change> {
    match command {
        Command::SetValue {
            row,
            column,
            path,
            value,
        } => {
            if path.is_empty() {
                return Err(invalid!(NoPathInCell));
            }
            let column_type = column_type(doc, &column)?;
            let leaf = type_at(&column_type, &path)
                .ok_or_else(|| invalid!(NoValueAt, column = column, path = describe(&path)))?
                .clone();
            let (parent, last) = path.split_at(path.len() - 1);
            // A list index at the end may append; a null fixed-size list is
            // filled with nulls first.
            if let PathPart::Index(index) = last[0] {
                let edit = ListEdit::Set {
                    index,
                    text: value.as_deref(),
                    item_type: &leaf,
                };
                let fixed = match type_at(&column_type, parent) {
                    Some(DataType::FixedSizeList(_, n)) => Some(*n as usize),
                    _ => None,
                };
                if fixed.is_some_and(|n| index >= n) {
                    return Err(invalid!(NoValueAt, column = column, path = describe(&path)));
                }
                return edit_cell(doc, row, column, |json| {
                    let json = match fixed {
                        Some(n) => fill_null_list(json, parent, n)?,
                        None => json.to_owned(),
                    };
                    edit_list(&json, parent, &edit)
                });
            }
            edit_cell(doc, row, column, |json| {
                set_nested(json, &path, value.as_deref(), &leaf)
            })
        }
        Command::InsertItem {
            row,
            column,
            path,
            index,
            key,
        } => match container(doc, &column, &path)? {
            Container::List => edit_cell(doc, row, column, |json| {
                edit_list(json, &path, &ListEdit::Insert(index))
            }),
            Container::Map(key_type) => {
                let key = key.ok_or_else(|| invalid!(EntryNeedsKey))?;
                let edit = MapEdit::Insert {
                    index,
                    key: &key,
                    key_type: &key_type,
                };
                edit_cell(doc, row, column, |json| edit_map(json, &path, &edit))
            }
            Container::Fixed => Err(invalid!(FixedListGrow)),
        },
        Command::RemoveItem {
            row,
            column,
            path,
            index,
        } => match container(doc, &column, &path)? {
            Container::List => edit_cell(doc, row, column, |json| {
                edit_list(json, &path, &ListEdit::Remove(index))
            }),
            Container::Map(_) => edit_cell(doc, row, column, |json| {
                edit_map(json, &path, &MapEdit::Remove(index))
            }),
            Container::Fixed => Err(invalid!(FixedListShrink)),
        },
        Command::SetKey {
            row,
            column,
            path,
            index,
            key,
        } => match container(doc, &column, &path)? {
            Container::Map(key_type) => {
                let edit = MapEdit::SetKey {
                    index,
                    key: &key,
                    key_type: &key_type,
                };
                edit_cell(doc, row, column, |json| edit_map(json, &path, &edit))
            }
            _ => Err(invalid!(NoMapAt, column = column, path = describe(&path))),
        },
        _ => unreachable!("not a nested value command"),
    }
}

fn column_type(doc: &Document, column: &str) -> Result<DataType> {
    Ok(doc
        .schema()
        .field_with_name(column)
        .map_err(|_| invalid!(ColumnNotFound, column = column))?
        .data_type()
        .clone())
}

/// A list or map that items are added to or removed from.
enum Container {
    List,
    /// A map, and its key type.
    Map(DataType),
    /// A fixed-size list: items can only be set.
    Fixed,
}

/// The list or map at `path` in `column`.
fn container(doc: &Document, column: &str, path: &[PathPart]) -> Result<Container> {
    let column_type = column_type(doc, column)?;
    match type_at(&column_type, path) {
        Some(DataType::List(_) | DataType::LargeList(_)) => Ok(Container::List),
        Some(DataType::FixedSizeList(..)) => Ok(Container::Fixed),
        Some(t @ DataType::Map(..)) => Ok(Container::Map(
            map_key_type(t).cloned().unwrap_or(DataType::Utf8),
        )),
        _ if path.is_empty() => Err(invalid!(NotListOrMapColumn, column = column)),
        _ => Err(invalid!(
            NotListOrMapAt,
            column = column,
            path = describe(path)
        )),
    }
}

/// A path as text, e.g. `orders[0].options{1}` (`{1}`: entry 1 of a map).
fn describe(path: &[PathPart]) -> String {
    let mut out = String::new();
    for part in path {
        match part {
            PathPart::Field(name) if out.is_empty() => out.push_str(name),
            PathPart::Field(name) => out.push_str(&format!(".{name}")),
            PathPart::Index(i) => out.push_str(&format!("[{i}]")),
            PathPart::Entry(i) => out.push_str(&format!("{{{i}}}")),
        }
    }
    out
}

/// Sets the cell at `row`, `column` to its JSON changed by `change`, as
/// [`Command::SetCell`] would.
fn edit_cell(
    doc: &Document,
    row: usize,
    column: String,
    change: impl FnOnce(&str) -> Result<String>,
) -> Result<Change> {
    let index = doc
        .schema()
        .index_of(&column)
        .map_err(|_| invalid!(ColumnNotFound, column = column))?;
    if row >= doc.num_rows() {
        return Err(invalid!(RowPastEnd, row = row + 1, rows = doc.num_rows()));
    }
    let batch = doc.read(row..row + 1)?;
    let json = value_json(batch.column(index).as_ref(), 0)?.unwrap_or_else(|| "null".into());
    let value = change(&json)?;
    cells::plan(
        doc,
        Command::SetCell {
            row,
            column,
            value: Some(value),
        },
    )
}
