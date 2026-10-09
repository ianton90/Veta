//! The controller: the only code that mutates a [`Document`].
//!
//! Every command is validated and turned into a reversible change, applied,
//! and recorded in the document's undo history.

use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray, new_null_array};
use arrow::compute::{CastOptions, cast_with_options};
use arrow::datatypes::DataType;

use crate::command::Command;
use crate::display::type_name;
use crate::error::{Error, Result};
use crate::history::Change;
use crate::model::{Document, KeyValue};
use crate::steps::{Step, normalize_ranges};

/// Validates `command` and applies it to `document`, recording it for undo.
///
/// On error the document is left unchanged.
pub fn execute(document: &mut Document, command: Command) -> Result<()> {
    let (label, change) = plan(document, command)?;
    change.apply(document)?;
    document.history.record(label, change);
    Ok(())
}

/// Reverts the last change. Returns `false` if there was nothing to undo.
pub fn undo(document: &mut Document) -> bool {
    let Some((_, change)) = document.history.pop_undo() else {
        return false;
    };
    change.revert(document);
    true
}

/// Re-applies the last undone change. Returns `Ok(false)` if there was
/// nothing to redo.
pub fn redo(document: &mut Document) -> Result<bool> {
    let Some((_, change)) = document.history.pop_redo() else {
        return Ok(false);
    };
    if let Err(e) = change.apply(document) {
        document.history.cancel_redo();
        return Err(e);
    }
    Ok(true)
}

/// Builds the change for `command` without touching the document.
fn plan(doc: &Document, command: Command) -> Result<(String, Change)> {
    match command {
        Command::SetMetadata { key, value } => {
            if key.is_empty() {
                return Err(Error::InvalidCommand("metadata key cannot be empty".into()));
            }
            let mut after = doc.metadata.clone();
            match after.key_value.iter_mut().find(|kv| kv.key == key) {
                Some(entry) => entry.value = value,
                None => after.key_value.push(KeyValue {
                    key: key.clone(),
                    value,
                }),
            }
            Ok((
                format!("Set metadata {key}"),
                Change::Metadata {
                    before: doc.metadata.clone(),
                    after,
                },
            ))
        }
        Command::RemoveMetadata { key } => {
            let mut after = doc.metadata.clone();
            let Some(index) = after.key_value.iter().position(|kv| kv.key == key) else {
                return Err(Error::InvalidCommand(format!("no metadata key {key:?}")));
            };
            after.key_value.remove(index);
            Ok((
                format!("Remove metadata {key}"),
                Change::Metadata {
                    before: doc.metadata.clone(),
                    after,
                },
            ))
        }
        Command::ReplaceMetadata(entries) => {
            for (i, kv) in entries.iter().enumerate() {
                if kv.key.trim().is_empty() {
                    return Err(invalid("metadata keys cannot be empty".into()));
                }
                if entries[..i].iter().any(|other| other.key == kv.key) {
                    return Err(invalid(format!("metadata key {:?} appears twice", kv.key)));
                }
            }
            let mut after = doc.metadata.clone();
            after.key_value = entries;
            Ok((
                "Edit metadata".into(),
                Change::Metadata {
                    before: doc.metadata.clone(),
                    after,
                },
            ))
        }
        Command::SetCell { row, column, value } => {
            let schema = doc.schema();
            let field = schema
                .field_with_name(&column)
                .map_err(|_| invalid(format!("no column named {column:?}")))?;
            if row >= doc.num_rows() {
                return Err(invalid(format!(
                    "row {} is past the last row ({})",
                    row + 1,
                    doc.num_rows()
                )));
            }
            let after = parse_value(value.as_deref(), field.data_type())?;
            if after.is_null(0) && !field.is_nullable() {
                return Err(invalid(format!("{column} cannot be empty")));
            }
            let label = format!("Edit {column} in row {}", row + 1);
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
            Ok((label, change))
        }
        Command::InsertRows { at, count } => Ok((
            format!("Insert {}", rows(count)),
            Change::PushStep(Step::InsertRows { at, count }),
        )),
        Command::DeleteRows { rows: ranges } => {
            let ranges = normalize_ranges(ranges);
            let count = ranges.iter().map(|r| r.len()).sum();
            Ok((
                format!("Delete {}", rows(count)),
                Change::PushStep(Step::DeleteRows { rows: ranges }),
            ))
        }
        Command::AddColumn {
            name,
            data_type,
            at,
        } => Ok((
            format!("Add column {name}"),
            Change::PushStep(Step::AddColumn {
                name,
                data_type,
                at,
            }),
        )),
        Command::RemoveColumns { names } => {
            let label = match names.as_slice() {
                [one] => format!("Remove column {one}"),
                _ => format!("Remove {} columns", names.len()),
            };
            Ok((label, Change::PushStep(Step::RemoveColumns { names })))
        }
        Command::RenameColumn { from, to } => {
            let label = format!("Rename {from} to {to}");
            let step = Change::PushStep(Step::RenameColumn {
                from: from.clone(),
                to: to.clone(),
            });
            // Keep the column's writer settings under its new name.
            let mut writer = doc.writer.clone();
            let prefix = format!("{from}.");
            let mut renamed = false;
            for (path, _) in &mut writer.columns {
                if *path == from {
                    *path = to.clone();
                    renamed = true;
                } else if let Some(rest) = path.strip_prefix(&prefix) {
                    *path = format!("{to}.{rest}");
                    renamed = true;
                }
            }
            let change = if renamed {
                Change::Batch(vec![
                    step,
                    Change::Writer {
                        before: doc.writer.clone(),
                        after: writer,
                    },
                ])
            } else {
                step
            };
            Ok((label, change))
        }
        Command::MoveColumn { name, to } => Ok((
            format!("Move column {name}"),
            Change::PushStep(Step::MoveColumn { name, to }),
        )),
        Command::SetWriterSettings(settings) => {
            if settings.max_row_group_rows == 0 {
                return Err(invalid("row group size must be at least 1".into()));
            }
            Ok((
                "Change writer settings".into(),
                Change::Writer {
                    before: doc.writer.clone(),
                    after: settings,
                },
            ))
        }
    }
}

fn invalid(message: String) -> Error {
    Error::InvalidCommand(message)
}

fn rows(n: usize) -> String {
    if n == 1 {
        "1 row".into()
    } else {
        format!("{n} rows")
    }
}

fn is_text(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    )
}

/// Parses user input as a one-element array of `data_type`.
fn parse_value(text: Option<&str>, data_type: &DataType) -> Result<ArrayRef> {
    let text = match text {
        None => return Ok(new_null_array(data_type, 1)),
        Some(t) if t.trim().is_empty() && !is_text(data_type) => {
            return Ok(new_null_array(data_type, 1));
        }
        Some(t) if is_text(data_type) => t,
        Some(t) => t.trim(),
    };
    let input: ArrayRef = Arc::new(StringArray::from(vec![text]));
    let options = CastOptions {
        safe: false,
        ..CastOptions::default()
    };
    cast_with_options(&input, data_type, &options)
        .map_err(|_| invalid(format!("{text:?} is not a valid {}", type_name(data_type))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpenOptions;
    use crate::display::format_batch;
    use veta_testkit::{TempDir, fixtures};

    fn set(key: &str, value: &str) -> Command {
        Command::SetMetadata {
            key: key.into(),
            value: Some(value.into()),
        }
    }

    fn remove(key: &str) -> Command {
        Command::RemoveMetadata { key: key.into() }
    }

    fn value<'a>(doc: &'a Document, key: &str) -> Option<&'a str> {
        doc.metadata().get(key)?.value.as_deref()
    }

    #[test]
    fn set_metadata_adds_then_replaces() {
        let mut doc = Document::new();
        execute(&mut doc, set("owner", "a")).unwrap();
        execute(&mut doc, set("owner", "b")).unwrap();

        let entries = &doc.metadata().key_value;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value.as_deref(), Some("b"));
    }

    #[test]
    fn rejected_commands_change_nothing() {
        let mut doc = Document::new();
        let err = execute(&mut doc, set("", "x")).unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(_)));
        assert!(doc.metadata().key_value.is_empty());
        assert!(execute(&mut doc, remove("missing")).is_err());
        assert!(!doc.history().can_undo());
        assert!(!doc.is_modified());
    }

    #[test]
    fn undo_and_redo() {
        let mut doc = Document::new();
        execute(&mut doc, set("owner", "a")).unwrap();
        execute(&mut doc, set("owner", "b")).unwrap();
        execute(&mut doc, remove("owner")).unwrap();
        assert_eq!(doc.history().undo_label(), Some("Remove metadata owner"));

        assert!(undo(&mut doc));
        assert_eq!(value(&doc, "owner"), Some("b"));
        assert!(undo(&mut doc));
        assert_eq!(value(&doc, "owner"), Some("a"));
        assert!(undo(&mut doc));
        assert_eq!(value(&doc, "owner"), None);
        assert!(!undo(&mut doc));

        assert!(redo(&mut doc).unwrap());
        assert_eq!(value(&doc, "owner"), Some("a"));
        assert_eq!(doc.history().redo_label(), Some("Set metadata owner"));

        // A new change discards the redo stack.
        execute(&mut doc, set("other", "x")).unwrap();
        assert!(!doc.history().can_redo());
        assert!(!redo(&mut doc).unwrap());
    }

    #[test]
    fn replace_metadata() {
        let mut doc = Document::new();
        execute(&mut doc, set("a", "1")).unwrap();
        let kv = |k: &str, v: Option<&str>| KeyValue {
            key: k.into(),
            value: v.map(str::to_owned),
        };
        let dup = vec![kv("x", None), kv("x", Some("2"))];
        assert!(execute(&mut doc, Command::ReplaceMetadata(dup)).is_err());
        assert!(execute(&mut doc, Command::ReplaceMetadata(vec![kv(" ", None)])).is_err());
        execute(
            &mut doc,
            Command::ReplaceMetadata(vec![kv("b", Some("2")), kv("c", None)]),
        )
        .unwrap();
        assert_eq!(doc.metadata().key_value.len(), 2);
        assert_eq!(value(&doc, "a"), None);
        undo(&mut doc);
        assert_eq!(value(&doc, "a"), Some("1"));
    }

    #[test]
    fn modified_tracks_the_saved_state() {
        let mut doc = Document::new();
        assert!(!doc.is_modified());
        execute(&mut doc, set("a", "1")).unwrap();
        assert!(doc.is_modified());
        undo(&mut doc);
        assert!(!doc.is_modified(), "back to the opened state");

        execute(&mut doc, set("a", "1")).unwrap();
        doc.history.mark_saved();
        assert!(!doc.is_modified());
        undo(&mut doc);
        assert!(doc.is_modified());
        redo(&mut doc).unwrap();
        assert!(!doc.is_modified());

        // Undo past the save point and branch: the saved state is gone.
        undo(&mut doc);
        execute(&mut doc, set("b", "2")).unwrap();
        assert!(doc.is_modified());
        undo(&mut doc);
        assert!(doc.is_modified());
    }

    /// Opens the mixed fixture: id (int64, required), name (utf8), score
    /// (float64), flag (bool); 1000 rows.
    fn mixed(dir: &TempDir) -> Document {
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);
        Document::open(&path, OpenOptions::default()).unwrap()
    }

    fn cell(doc: &Document, row: usize, column: &str) -> Option<String> {
        let index = doc.schema().index_of(column).unwrap();
        format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][index].clone()
    }

    fn set_cell(row: usize, column: &str, value: Option<&str>) -> Command {
        Command::SetCell {
            row,
            column: column.into(),
            value: value.map(str::to_owned),
        }
    }

    #[test]
    fn set_cell_parses_and_merges_into_one_step() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        execute(&mut doc, set_cell(1, "score", Some(" 2.25 "))).unwrap();
        execute(&mut doc, set_cell(2, "name", Some("  spaced "))).unwrap();
        execute(&mut doc, set_cell(3, "flag", Some("true"))).unwrap();
        execute(&mut doc, set_cell(4, "score", Some(""))).unwrap();
        execute(&mut doc, set_cell(1, "score", Some("9"))).unwrap();

        assert_eq!(doc.steps().len(), 1, "consecutive edits share a step");
        assert_eq!(cell(&doc, 1, "score").as_deref(), Some("9.0"));
        assert_eq!(cell(&doc, 2, "name").as_deref(), Some("  spaced "));
        assert_eq!(cell(&doc, 3, "flag").as_deref(), Some("true"));
        assert_eq!(cell(&doc, 4, "score"), None);

        // Undo goes back one edit at a time, including the overwrite.
        undo(&mut doc);
        assert_eq!(cell(&doc, 1, "score").as_deref(), Some("2.25"));
        for _ in 0..4 {
            undo(&mut doc);
        }
        assert!(doc.steps().is_empty());
        assert_eq!(cell(&doc, 1, "score").as_deref(), Some("0.5"));
        assert!(!doc.is_modified());
    }

    #[test]
    fn set_cell_rejects_bad_input() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let err = execute(&mut doc, set_cell(0, "score", Some("abc"))).unwrap_err();
        assert!(err.to_string().contains("not a valid float64"), "{err}");
        assert!(
            execute(&mut doc, set_cell(0, "id", None)).is_err(),
            "required"
        );
        assert!(
            execute(&mut doc, set_cell(0, "id", Some(""))).is_err(),
            "required"
        );
        assert!(execute(&mut doc, set_cell(5000, "id", Some("1"))).is_err());
        assert!(execute(&mut doc, set_cell(0, "nope", Some("1"))).is_err());
        assert!(doc.steps().is_empty());
    }

    #[test]
    fn row_and_column_commands_undo() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let commands = vec![
            Command::InsertRows { at: 0, count: 2 },
            Command::DeleteRows {
                rows: vec![10..20, 5..12],
            },
            Command::AddColumn {
                name: "new".into(),
                data_type: DataType::Int32,
                at: 4,
            },
            Command::RenameColumn {
                from: "name".into(),
                to: "label".into(),
            },
            Command::MoveColumn {
                name: "flag".into(),
                to: 0,
            },
            Command::RemoveColumns {
                names: vec!["score".into()],
            },
        ];
        for command in commands {
            execute(&mut doc, command).unwrap();
        }
        assert_eq!(doc.num_rows(), 1000 + 2 - 15);
        let names: Vec<_> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(names, ["flag", "id", "label", "new"]);
        assert_eq!(cell(&doc, 0, "id"), None, "inserted row");
        assert_eq!(cell(&doc, 5, "id").as_deref(), Some("18"));

        while undo(&mut doc) {}
        assert_eq!(doc.num_rows(), 1000);
        assert_eq!(doc.num_columns(), 4);
        assert_eq!(cell(&doc, 0, "id").as_deref(), Some("0"));
        assert!(!doc.is_modified());
    }

    #[test]
    fn rename_carries_writer_settings() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let zstd = doc.writer_settings().column("id").compression;
        execute(
            &mut doc,
            Command::RenameColumn {
                from: "id".into(),
                to: "key".into(),
            },
        )
        .unwrap();
        assert_eq!(doc.writer_settings().column("key").compression, zstd);
        undo(&mut doc);
        assert_eq!(doc.writer_settings().column("id").compression, zstd);
        assert!(
            doc.writer_settings()
                .columns
                .iter()
                .all(|(p, _)| p != "key")
        );
    }

    #[test]
    fn writer_settings_command() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let mut settings = doc.writer_settings().clone();
        settings.max_row_group_rows = 0;
        assert!(execute(&mut doc, Command::SetWriterSettings(settings.clone())).is_err());
        settings.max_row_group_rows = 50;
        execute(&mut doc, Command::SetWriterSettings(settings)).unwrap();
        assert_eq!(doc.writer_settings().max_row_group_rows, 50);
        undo(&mut doc);
        assert_eq!(
            doc.writer_settings().max_row_group_rows,
            fixtures::MIXED_SETTINGS_ROW_GROUP
        );
    }

    #[test]
    fn failed_step_is_not_recorded() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let err = execute(&mut doc, Command::InsertRows { at: 5000, count: 1 });
        assert!(err.is_err());
        assert!(!doc.history().can_undo());
    }
}
