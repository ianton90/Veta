//! Cell commands: setting one cell and pasting blocks of values.

use super::*;
use crate::error::invalid;

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<Change> {
    match command {
        Command::SetCell { row, column, value } => {
            let schema = doc.schema();
            let field = schema
                .field_with_name(&column)
                .map_err(|_| invalid!(ColumnNotFound, column = column))?;
            if row >= doc.num_rows() {
                return Err(invalid!(RowPastEnd, row = row + 1, rows = doc.num_rows()));
            }
            let after = parse_value(value.as_deref(), field.data_type())?;
            if after.is_null(0) && !field.is_nullable() {
                return Err(invalid!(NotNullable, column = column));
            }
            let change = match doc.steps().last() {
                Some(Step::EditCells(edits)) => Change::EditCell {
                    before: edits.get(&column, row).cloned(),
                    column,
                    row,
                    after,
                    new_step: false,
                },
                _ => Change::EditCell {
                    column,
                    row,
                    before: None,
                    after,
                    new_step: true,
                },
            };
            Ok(change)
        }
        Command::PasteCells {
            row,
            column,
            values,
        } => {
            let schema = doc.schema();
            let width = values.iter().map(Vec::len).max().unwrap_or(0);
            if width == 0 {
                return Err(invalid!(NothingToPaste));
            }
            if column + width > schema.fields().len() {
                return Err(invalid!(
                    PasteTooWide,
                    width = width,
                    fit = schema.fields().len().saturating_sub(column),
                    column = column + 1
                ));
            }
            let mut changes = Vec::new();
            let end = row + values.len();
            if end > doc.num_rows() {
                changes.push(Change::PushStep(Step::InsertRows {
                    at: doc.num_rows(),
                    count: end - doc.num_rows(),
                }));
            }
            // Cells join the last step if it already edits cells.
            let existing = match (changes.is_empty(), doc.steps().last()) {
                (true, Some(Step::EditCells(edits))) => Some(edits),
                _ => None,
            };
            let mut new_step = existing.is_none();
            for (i, line) in values.iter().enumerate() {
                for (j, value) in line.iter().enumerate() {
                    let field = schema.field(column + j);
                    let name = field.name().clone();
                    let r = row + i;
                    let after = parse_value(value.as_deref(), field.data_type())
                        .map_err(|e| invalid!(RowColumn, row = r + 1, column = name; cause = e))?;
                    if after.is_null(0) && !field.is_nullable() {
                        return Err(invalid!(NotNullableRow, column = name, row = r + 1));
                    }
                    changes.push(Change::EditCell {
                        before: existing.and_then(|e| e.get(&name, r).cloned()),
                        column: name,
                        row: r,
                        after,
                        new_step,
                    });
                    new_step = false;
                }
            }
            Ok(Change::Batch(changes))
        }
        Command::PasteFields {
            row,
            targets,
            values,
        } => {
            use crate::display::{PathPart, field_type, value_json};
            let schema = doc.schema();
            let width = values.iter().map(Vec::len).max().unwrap_or(0);
            if width == 0 {
                return Err(invalid!(NothingToPaste));
            }
            if width > targets.len() {
                return Err(invalid!(
                    PasteTooWideThere,
                    width = width,
                    fit = targets.len()
                ));
            }
            let mut changes = Vec::new();
            let end = row + values.len();
            let existing_rows = row..end.min(doc.num_rows());
            if end > doc.num_rows() {
                changes.push(Change::PushStep(Step::InsertRows {
                    at: doc.num_rows(),
                    count: end - doc.num_rows(),
                }));
            }
            let current = if existing_rows.is_empty() {
                None
            } else {
                Some(doc.read(existing_rows)?)
            };
            let existing = match (changes.is_empty(), doc.steps().last()) {
                (true, Some(Step::EditCells(edits))) => Some(edits),
                _ => None,
            };
            let mut new_step = existing.is_none();
            for (i, line) in values.iter().enumerate() {
                let r = row + i;
                // Fields of one struct cell are set one after the other.
                let mut cells: Vec<(usize, String)> = Vec::new();
                for (j, value) in line.iter().enumerate() {
                    let (c, path) = &targets[j];
                    let field = schema
                        .fields()
                        .get(*c)
                        .ok_or_else(|| invalid!(NoColumnAt, column = c + 1))?;
                    let name = field.name();
                    let leaf = field_type(field.data_type(), path).ok_or_else(|| {
                        invalid!(FieldNotFound, at = name, field = path.join("."))
                    })?;
                    if path.is_empty() {
                        let after = parse_value(value.as_deref(), leaf).map_err(
                            |e| invalid!(RowColumn, row = r + 1, column = name; cause = e),
                        )?;
                        if after.is_null(0) && !field.is_nullable() {
                            return Err(invalid!(NotNullableRow, column = name, row = r + 1));
                        }
                        cells.retain(|(cc, _)| cc != c);
                        changes.push(Change::EditCell {
                            before: existing.and_then(|e| e.get(name, r).cloned()),
                            column: name.clone(),
                            row: r,
                            after,
                            new_step,
                        });
                        new_step = false;
                        continue;
                    }
                    let json = match cells.iter().position(|(cc, _)| cc == c) {
                        Some(k) => cells.remove(k).1,
                        None => current
                            .as_ref()
                            .filter(|b| i < b.num_rows())
                            .and_then(|b| value_json(b.column(*c).as_ref(), i).ok().flatten())
                            .unwrap_or_else(|| "null".into()),
                    };
                    let pointer: Vec<_> = path.iter().cloned().map(PathPart::Field).collect();
                    let json = crate::values::set_nested(&json, &pointer, value.as_deref(), leaf)
                        .map_err(
                        |e| invalid!(RowColumn, row = r + 1, column = name; cause = e),
                    )?;
                    cells.push((*c, json));
                }
                for (c, json) in cells {
                    let field = schema.field(c);
                    let after = parse_value(Some(&json), field.data_type())?;
                    changes.push(Change::EditCell {
                        before: existing.and_then(|e| e.get(field.name(), r).cloned()),
                        column: field.name().clone(),
                        row: r,
                        after,
                        new_step,
                    });
                    new_step = false;
                }
            }
            Ok(Change::Batch(changes))
        }
        other => Err(invalid!(
            WrongArea,
            area = "cells",
            command = format!("{other:?}")
        )),
    }
}
