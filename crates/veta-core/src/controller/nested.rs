//! Nested value commands: struct fields and list items inside a cell. Each
//! becomes an edit of the whole cell.

use super::*;
use arrow::datatypes::DataType;

use crate::display::{PathPart, field_type, value_json};
use crate::values::{ListEdit, edit_list, set_nested};

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<(String, Change)> {
    match command {
        Command::SetField {
            row,
            column,
            path,
            value,
        } => {
            let leaf = column_type(doc, &column)?;
            let leaf = field_type(&leaf, &path)
                .ok_or_else(|| invalid(format!("{column} has no field {}", path.join("."))))?
                .clone();
            let pointer = pointer(&path);
            edit_cell(doc, row, column, |json| {
                set_nested(json, &pointer, value.as_deref(), &leaf)
            })
        }
        Command::SetListItem {
            row,
            column,
            list,
            index,
            value,
        } => {
            let item_type = item_type(doc, &column, &list)?;
            let edit = ListEdit::Set {
                index,
                text: value.as_deref(),
                item_type: &item_type,
            };
            edit_cell(doc, row, column, |json| {
                edit_list(json, &pointer(&list), &edit)
            })
        }
        Command::InsertListItem {
            row,
            column,
            list,
            index,
        } => {
            item_type(doc, &column, &list)?;
            edit_cell(doc, row, column, |json| {
                edit_list(json, &pointer(&list), &ListEdit::Insert(index))
            })
        }
        Command::RemoveListItem {
            row,
            column,
            list,
            index,
        } => {
            item_type(doc, &column, &list)?;
            edit_cell(doc, row, column, |json| {
                edit_list(json, &pointer(&list), &ListEdit::Remove(index))
            })
        }
        _ => unreachable!("not a nested value command"),
    }
}

fn column_type(doc: &Document, column: &str) -> Result<DataType> {
    Ok(doc
        .schema()
        .field_with_name(column)
        .map_err(|_| invalid(format!("no column named {column:?}")))?
        .data_type()
        .clone())
}

/// The item type of the list at `list` in `column`.
fn item_type(doc: &Document, column: &str, list: &[String]) -> Result<DataType> {
    let data_type = column_type(doc, column)?;
    match field_type(&data_type, list) {
        Some(DataType::List(item) | DataType::LargeList(item)) => Ok(item.data_type().clone()),
        _ => Err(invalid(format!(
            "{} is not a list",
            std::iter::once(column)
                .chain(list.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(".")
        ))),
    }
}

fn pointer(path: &[String]) -> Vec<PathPart> {
    path.iter().cloned().map(PathPart::Field).collect()
}

/// Sets the cell at `row`, `column` to its JSON changed by `change`, as
/// [`Command::SetCell`] would.
fn edit_cell(
    doc: &Document,
    row: usize,
    column: String,
    change: impl FnOnce(&str) -> Result<String>,
) -> Result<(String, Change)> {
    let index = doc
        .schema()
        .index_of(&column)
        .map_err(|_| invalid(format!("no column named {column:?}")))?;
    if row >= doc.num_rows() {
        return Err(invalid(format!(
            "row {} is past the last row ({})",
            row + 1,
            doc.num_rows()
        )));
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
