//! The controller: the only code that mutates a [`Document`].
//!
//! Every command is validated and turned into a reversible change, applied,
//! and recorded in the document's undo history.

use crate::command::Command;
use crate::error::{Error, Result};
use crate::history::Change;
use crate::model::{Document, KeyValue};
use crate::steps::{Step, normalize_ranges};
use crate::values::parse_value;

/// Validates `command` and applies it to `document`, recording it for undo.
///
/// On error the document is left unchanged.
pub fn execute(document: &mut Document, command: Command) -> Result<()> {
    let (label, change) = plan(document, command)?;
    change.apply(document)?;
    document.bump_revision();
    document.history.record(label, change);
    Ok(())
}

/// Like [`execute`], but folds the change into the last one in the
/// history, so they undo as one (for follow-up changes the app makes on its
/// own).
pub fn amend(document: &mut Document, command: Command) -> Result<()> {
    let (_, change) = plan(document, command)?;
    change.apply(document)?;
    document.bump_revision();
    document.history.amend(change);
    Ok(())
}

/// Reverts the last change. Returns `false` if there was nothing to undo.
pub fn undo(document: &mut Document) -> bool {
    let Some((_, change)) = document.history.pop_undo() else {
        return false;
    };
    change.revert(document);
    document.bump_revision();
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
    document.bump_revision();
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
        Command::ReplaceColumnMetadata { column, entries } => {
            if doc.schema().field_with_name(&column).is_err() {
                return Err(invalid(format!("no column named {column:?}")));
            }
            check_keys(&entries)?;
            let mut after = doc.metadata.clone();
            after.columns.retain(|(name, _)| *name != column);
            after.columns.push((column.clone(), entries));
            Ok((
                format!("Edit metadata of {column}"),
                Change::Metadata {
                    before: doc.metadata.clone(),
                    after,
                },
            ))
        }
        Command::ReplaceMetadata(entries) => {
            check_keys(&entries)?;
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
        Command::PasteCells {
            row,
            column,
            values,
        } => {
            let schema = doc.schema();
            let width = values.iter().map(Vec::len).max().unwrap_or(0);
            if width == 0 {
                return Err(invalid("there is nothing to paste".into()));
            }
            if column + width > schema.fields().len() {
                return Err(invalid(format!(
                    "the pasted values are {width} columns wide; only {} fit from column {}",
                    schema.fields().len().saturating_sub(column),
                    column + 1
                )));
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
            let mut count = 0;
            for (i, line) in values.iter().enumerate() {
                for (j, value) in line.iter().enumerate() {
                    let field = schema.field(column + j);
                    let name = field.name().clone();
                    let r = row + i;
                    let after = parse_value(value.as_deref(), field.data_type())
                        .map_err(|e| invalid(format!("row {}, {name}: {e}", r + 1)))?;
                    if after.is_null(0) && !field.is_nullable() {
                        return Err(invalid(format!("{name} cannot be empty (row {})", r + 1)));
                    }
                    changes.push(Change::EditCell {
                        before: existing.and_then(|e| e.get(&name, r).cloned()),
                        column: name,
                        row: r,
                        after,
                        new_step,
                    });
                    new_step = false;
                    count += 1;
                }
            }
            let label = if count == 1 {
                "Paste 1 cell".to_owned()
            } else {
                format!("Paste {count} cells")
            };
            Ok((label, Change::Batch(changes)))
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
        Command::KeepColumns { names } => {
            let step = Step::KeepColumns { names };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::RenameColumn { from, to } => {
            let label = format!("Rename {from} to {to}");
            let step = Change::PushStep(Step::RenameColumn {
                from: from.clone(),
                to: to.clone(),
            });
            // Keep the column's writer settings and edited metadata under
            // its new name.
            let mut metadata = doc.metadata.clone();
            for (name, _) in &mut metadata.columns {
                if *name == from {
                    *name = to.clone();
                }
            }
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
            let mut changes = vec![step];
            if renamed {
                changes.push(Change::Writer {
                    before: doc.writer.clone(),
                    after: writer,
                });
            }
            if metadata != doc.metadata {
                changes.push(Change::Metadata {
                    before: doc.metadata.clone(),
                    after: metadata,
                });
            }
            let change = if changes.len() == 1 {
                changes.remove(0)
            } else {
                Change::Batch(changes)
            };
            Ok((label, change))
        }
        Command::MoveColumn { name, to } => Ok((
            format!("Move column {name}"),
            Change::PushStep(Step::MoveColumn { name, to }),
        )),
        Command::Filter(filter) => Ok((filter.describe(), Change::PushStep(Step::Filter(filter)))),
        Command::Sort(sort) => Ok((sort.describe(), Change::PushStep(Step::Sort(sort)))),
        Command::Cast(cast) => Ok((cast.describe(), Change::PushStep(Step::Cast(cast)))),
        Command::Replace(replace) => {
            Ok((replace.describe(), Change::PushStep(Step::Replace(replace))))
        }
        Command::FormatText { columns, op } => {
            let step = Step::FormatText { columns, op };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::Fill(fill) => Ok((fill.describe(), Change::PushStep(Step::Fill(fill)))),
        Command::Split(split) => Ok((split.describe(), Change::PushStep(Step::Split(split)))),
        Command::ExpandStruct { column, all_levels } => {
            let step = Step::ExpandStruct { column, all_levels };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::ExplodeList { column } => {
            let step = Step::ExplodeList { column };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::CollectList { keys, columns } => {
            let step = Step::CollectList { keys, columns };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::CombineStruct { columns, name } => {
            let step = Step::CombineStruct { columns, name };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::Merge(merge) => Ok((merge.describe(), Change::PushStep(Step::Merge(merge)))),
        Command::CustomColumn(custom) => Ok((
            custom.describe(),
            Change::PushStep(Step::CustomColumn(custom)),
        )),
        Command::RemoveStep { index } => {
            let mut after = doc.steps().to_vec();
            if index >= after.len() {
                return Err(invalid(format!("there is no step {}", index + 1)));
            }
            let removed = after.remove(index);
            Ok((
                format!("Remove step: {}", removed.describe()),
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after,
                    keep: index,
                },
            ))
        }
        Command::MoveStep { from, to } => {
            let mut after = doc.steps().to_vec();
            if from >= after.len() || to >= after.len() {
                return Err(invalid("step position out of range".into()));
            }
            let step = after.remove(from);
            let label = format!("Move step: {}", step.describe());
            after.insert(to, step);
            Ok((
                label,
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after,
                    keep: from.min(to),
                },
            ))
        }
        Command::AppendSteps { steps } => {
            if steps.is_empty() {
                return Err(invalid("there are no steps to add".into()));
            }
            let before = doc.steps().to_vec();
            let keep = before.len();
            let mut after = before.clone();
            let label = match steps.as_slice() {
                [one] => one.describe(),
                many => format!("Add {} steps", many.len()),
            };
            after.extend(steps);
            Ok((
                label,
                Change::Steps {
                    before,
                    after,
                    keep,
                },
            ))
        }
        Command::ReplaceStep { index, step } => {
            let mut after = doc.steps().to_vec();
            let Some(slot) = after.get_mut(index) else {
                return Err(invalid(format!("there is no step {}", index + 1)));
            };
            let label = format!("Edit step: {}", step.describe());
            *slot = step;
            Ok((
                label,
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after,
                    keep: index,
                },
            ))
        }
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

fn check_keys(entries: &[KeyValue]) -> Result<()> {
    for (i, kv) in entries.iter().enumerate() {
        if kv.key.trim().is_empty() {
            return Err(invalid("metadata keys cannot be empty".into()));
        }
        if entries[..i].iter().any(|other| other.key == kv.key) {
            return Err(invalid(format!("metadata key {:?} appears twice", kv.key)));
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpenOptions;
    use crate::display::format_batch;
    use arrow::datatypes::DataType;
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
    fn step_commands_and_broken_steps() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let rename = |to: &str| Command::RenameColumn {
            from: "name".into(),
            to: to.into(),
        };
        execute(&mut doc, rename("label")).unwrap();
        execute(
            &mut doc,
            Command::MoveColumn {
                name: "label".into(),
                to: 0,
            },
        )
        .unwrap();
        execute(&mut doc, Command::InsertRows { at: 0, count: 1 }).unwrap();

        // Removing the rename breaks the move that refers to "label".
        execute(&mut doc, Command::RemoveStep { index: 0 }).unwrap();
        assert!(matches!(
            doc.status(),
            crate::steps::Status::Broken { step: 0, .. }
        ));
        assert_eq!(doc.steps().len(), 2);
        assert!(doc.save_job(None).is_err(), "can't save while broken");
        assert!(
            execute(&mut doc, Command::InsertRows { at: 0, count: 1 }).is_err(),
            "can't add steps while broken"
        );
        undo(&mut doc);
        assert_eq!(doc.status(), &crate::steps::Status::Ready);
        assert_eq!(doc.schema().field(0).name(), "label");

        // Move the insert before the rename, then edit the rename.
        execute(&mut doc, Command::MoveStep { from: 2, to: 0 }).unwrap();
        assert!(matches!(doc.steps()[0], Step::InsertRows { .. }));
        let Step::RenameColumn { from, .. } = doc.steps()[1].clone() else {
            panic!("expected rename");
        };
        execute(
            &mut doc,
            Command::ReplaceStep {
                index: 1,
                step: Step::RenameColumn {
                    from,
                    to: "title".into(),
                },
            },
        )
        .unwrap();
        assert!(matches!(
            doc.status(),
            crate::steps::Status::Broken { step: 2, .. }
        ));
        assert!(execute(&mut doc, Command::RemoveStep { index: 9 }).is_err());
    }

    #[test]
    fn filter_command_and_save_with_pending_step() {
        use crate::steps::{Condition, Filter, FilterOp, Status};
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let filter = Filter {
            conditions: vec![Condition {
                column: "name".into(),
                op: FilterOp::Equals,
                value: "BO".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        };
        execute(&mut doc, Command::Filter(filter)).unwrap();
        assert_eq!(doc.status(), &Status::Computing { step: 0 });
        // Saving computes the pending step.
        let out = dir.join("out.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert!(saved.num_rows() > 0 && saved.num_rows() < 1000);
        doc.compute_all().unwrap();
        assert_eq!(doc.num_rows(), saved.num_rows());
        undo(&mut doc);
        assert_eq!(doc.num_rows(), 1000);

        let bad = Filter {
            conditions: vec![Condition {
                column: "score".into(),
                op: FilterOp::Less,
                value: "abc".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        };
        assert!(execute(&mut doc, Command::Filter(bad)).is_err());
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn sort_command_in_memory_and_paged() {
        use crate::steps::{Sort, SortKey, Status};
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);
        let sort = Sort {
            keys: vec![
                SortKey {
                    column: "flag".into(),
                    descending: false,
                    nulls_first: false,
                },
                SortKey {
                    column: "score".into(),
                    descending: true,
                    nulls_first: false,
                },
            ],
        };
        let mut results = Vec::new();
        for options in [OpenOptions::default(), OpenOptions::paged()] {
            let mut doc = Document::open(&path, options).unwrap();
            execute(&mut doc, Command::Sort(sort.clone())).unwrap();
            assert_eq!(doc.status(), &Status::Computing { step: 0 });
            doc.compute_all().unwrap();
            assert_eq!(doc.num_rows(), 1000);
            // false first; within it, scores high to low with nulls last.
            assert_eq!(cell(&doc, 0, "flag").as_deref(), Some("false"));
            assert_eq!(cell(&doc, 0, "score").as_deref(), Some("499.5"));
            assert_eq!(cell(&doc, 499, "flag").as_deref(), Some("false"));
            assert_eq!(cell(&doc, 499, "score"), None);
            assert_eq!(cell(&doc, 500, "flag").as_deref(), Some("true"));
            // Later steps work on the sorted rows.
            execute(&mut doc, Command::DeleteRows { rows: vec![0..1] }).unwrap();
            assert_eq!(cell(&doc, 0, "score").as_deref(), Some("498.5"));
            let out = dir.join("sorted.parquet");
            doc.save(Some(out.clone())).unwrap();
            let saved = Document::open(&out, OpenOptions::default()).unwrap();
            results.push(format_batch(&saved.read(0..999).unwrap()).unwrap());
        }
        assert_eq!(results[0], results[1]);
    }

    #[test]
    fn cast_command_with_error_column() {
        use crate::steps::{Cast, CastOptions, OnError};
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        // score (float) → text with a comma decimal, then back to a number.
        execute(
            &mut doc,
            Command::Cast(Cast {
                column: "score".into(),
                to: DataType::Utf8,
                options: CastOptions::default(),
            }),
        )
        .unwrap();
        assert_eq!(cell(&doc, 3, "score").as_deref(), Some("1.5"));
        execute(
            &mut doc,
            Command::SetCell {
                row: 3,
                column: "score".into(),
                value: Some("1.234,5".into()),
            },
        )
        .unwrap();
        execute(
            &mut doc,
            Command::Cast(Cast {
                column: "score".into(),
                to: DataType::Float64,
                options: CastOptions {
                    decimal: ',',
                    thousands: Some('.'),
                    on_error: OnError::ErrorColumn,
                    ..CastOptions::default()
                },
            }),
        )
        .unwrap();
        assert_eq!(cell(&doc, 3, "score").as_deref(), Some("1234.5"));
        // "2.5" reads as 25 with these separators; nothing fails here.
        assert_eq!(cell(&doc, 5, "score").as_deref(), Some("25.0"));
        assert_eq!(cell(&doc, 5, "score errors"), None);

        // name → int: every non-null name fails into the error column.
        let mut cast = Cast {
            column: "name".into(),
            to: DataType::Int64,
            options: CastOptions {
                on_error: OnError::ErrorColumn,
                ..CastOptions::default()
            },
        };
        execute(&mut doc, Command::Cast(cast.clone())).unwrap();
        assert_eq!(cell(&doc, 1, "name"), None);
        assert_eq!(cell(&doc, 1, "name errors").as_deref(), Some("bo"));
        let out = dir.join("cast.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(saved.schema().field(2).name(), "name errors");

        undo(&mut doc);
        cast.options.on_error = OnError::Fail;
        execute(&mut doc, Command::Cast(cast)).unwrap();
        assert!(doc.compute_all().is_err(), "failing values break the step");
        assert!(matches!(doc.status(), crate::steps::Status::Broken { .. }));
        assert!(doc.save(Some(dir.join("fail.parquet"))).is_err());
    }

    #[test]
    fn replace_format_and_fill_commands() {
        use crate::steps::{Fill, Replace, Status, TextOp};
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);
        for options in [OpenOptions::default(), OpenOptions::paged()] {
            let mut doc = Document::open(&path, options).unwrap();
            execute(
                &mut doc,
                Command::Replace(Replace {
                    column: "name".into(),
                    find: Some("o".into()),
                    with: Some("0".into()),
                    whole_cell: false,
                }),
            )
            .unwrap();
            assert_eq!(cell(&doc, 1, "name").as_deref(), Some("b0"));
            execute(
                &mut doc,
                Command::FormatText {
                    columns: vec!["name".into()],
                    op: TextOp::Upper,
                },
            )
            .unwrap();
            assert_eq!(cell(&doc, 2, "name").as_deref(), Some("CY"));
            execute(
                &mut doc,
                Command::Fill(Fill {
                    columns: vec!["name".into(), "score".into()],
                    up: false,
                }),
            )
            .unwrap();
            assert_eq!(doc.status(), &Status::Computing { step: 2 });
            doc.compute_all().unwrap();
            // Row 7's name and row 11's score were null.
            assert_eq!(cell(&doc, 7, "name").as_deref(), Some("B0"));
            assert_eq!(cell(&doc, 11, "score").as_deref(), Some("5.0"));
            assert_eq!(cell(&doc, 0, "name"), None, "nothing above row 0");
            execute(
                &mut doc,
                Command::Replace(Replace {
                    column: "score".into(),
                    find: None,
                    with: Some("-1".into()),
                    whole_cell: true,
                }),
            )
            .unwrap();
            assert_eq!(cell(&doc, 0, "score").as_deref(), Some("-1.0"));
            assert!(
                execute(
                    &mut doc,
                    Command::FormatText {
                        columns: vec!["score".into()],
                        op: TextOp::Trim,
                    },
                )
                .is_err()
            );
        }
    }

    #[test]
    fn custom_column_command() {
        use crate::steps::CustomColumn;
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let custom = |name: &str, expression: &str, at| {
            Command::CustomColumn(CustomColumn {
                name: name.into(),
                expression: expression.into(),
                at,
            })
        };
        execute(&mut doc, custom("n", "row() * 10 + id", 0)).unwrap();
        assert_eq!(doc.schema().field(0).name(), "n");
        // row() counts from the step's first row, whatever window is read.
        assert_eq!(cell(&doc, 0, "n").as_deref(), Some("10"));
        assert_eq!(cell(&doc, 500, "n").as_deref(), Some("5510"));
        execute(
            &mut doc,
            custom(
                "label",
                "if(flag, upper(name), \"-\") & \"/\" & text(score)",
                5,
            ),
        )
        .unwrap();
        assert_eq!(cell(&doc, 2, "label").as_deref(), Some("CY/1.0"));
        assert_eq!(cell(&doc, 0, "label"), None, "null name and score");
        assert!(execute(&mut doc, custom("bad", "name * 2", 0)).is_err());
        assert!(execute(&mut doc, custom("n", "1", 0)).is_err());
        let out = dir.join("custom.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(saved.num_columns(), 6);
    }

    #[test]
    fn recipe_round_trip_onto_another_document() {
        use crate::steps::{Sort, SortKey};
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        execute(
            &mut doc,
            Command::RenameColumn {
                from: "name".into(),
                to: "label".into(),
            },
        )
        .unwrap();
        execute(
            &mut doc,
            Command::SetCell {
                row: 0,
                column: "score".into(),
                value: Some("0.1".into()),
            },
        )
        .unwrap();
        execute(
            &mut doc,
            Command::Sort(Sort {
                keys: vec![SortKey {
                    column: "score".into(),
                    descending: true,
                    nulls_first: false,
                }],
            }),
        )
        .unwrap();
        doc.compute_all().unwrap();
        let text = crate::recipe::to_json(doc.steps(), &Default::default()).unwrap();

        let mut other = mixed(&dir);
        let steps = crate::recipe::from_json(&text, &other.schema()).unwrap();
        execute(&mut other, Command::AppendSteps { steps }).unwrap();
        other.compute_all().unwrap();
        let all = |d: &Document| format_batch(&d.read(0..1000).unwrap()).unwrap();
        assert_eq!(all(&doc), all(&other));
        // One undo removes the whole recipe.
        assert!(undo(&mut other));
        assert!(other.steps().is_empty());
        assert!(execute(&mut other, Command::AppendSteps { steps: vec![] }).is_err());
    }

    #[test]
    fn keep_columns_keeps_table_order() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        execute(
            &mut doc,
            Command::KeepColumns {
                names: vec!["flag".into(), "id".into()],
            },
        )
        .unwrap();
        let names: Vec<_> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(names, ["id", "flag"]);
        assert_eq!(cell(&doc, 3, "flag").as_deref(), Some("false"));
        assert!(
            execute(
                &mut doc,
                Command::KeepColumns {
                    names: vec!["nope".into()]
                }
            )
            .is_err()
        );
        assert!(execute(&mut doc, Command::KeepColumns { names: vec![] }).is_err());
    }

    #[test]
    fn remove_empty_column_checks_every_row() {
        use crate::steps::{Cast, CastOptions, OnError, Status};
        let dir = TempDir::new();
        let cast = |column: &str, to| Cast {
            column: column.into(),
            to,
            options: CastOptions {
                on_error: OnError::ErrorColumn,
                ..CastOptions::default()
            },
        };
        let remove = |name: &str| Step::RemoveEmptyColumn { name: name.into() };

        // No errors: the error column goes.
        let mut doc = mixed(&dir);
        let steps = vec![Step::Cast(cast("id", DataType::Utf8)), remove("id errors")];
        execute(&mut doc, Command::AppendSteps { steps }).unwrap();
        assert_eq!(doc.status(), &Status::Computing { step: 1 });
        doc.compute_all().unwrap();
        assert_eq!(doc.num_columns(), 4);
        assert_eq!(cell(&doc, 2, "id").as_deref(), Some("2"));

        // Errors: the check fails, saying why.
        let mut doc = mixed(&dir);
        let steps = vec![
            Step::Cast(cast("name", DataType::Int64)),
            remove("name errors"),
        ];
        execute(&mut doc, Command::AppendSteps { steps }).unwrap();
        let err = doc.compute_all().unwrap_err().to_string();
        assert!(
            err.contains("errors isn't empty") && err.contains("\"bo\""),
            "{err}"
        );
        // The app drops the check, folded into the same undo entry.
        amend(&mut doc, Command::RemoveStep { index: 1 }).unwrap();
        assert_eq!(doc.status(), &Status::Ready);
        assert_eq!(doc.num_columns(), 5);
        assert!(undo(&mut doc));
        assert!(doc.steps().is_empty());
        assert!(!doc.history().can_undo());
    }

    #[test]
    fn nested_cells_edit_as_json_and_expand_explode() {
        use crate::steps::Status;
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        for options in [OpenOptions::default(), OpenOptions::paged()] {
            let mut doc = Document::open(&path, options).unwrap();
            execute(
                &mut doc,
                Command::SetCell {
                    row: 2,
                    column: "point".into(),
                    value: Some(r#"{"x": 10, "y": 20}"#.into()),
                },
            )
            .unwrap();
            assert_eq!(cell(&doc, 2, "point").as_deref(), Some("{x: 10, y: 20}"));
            let bad = execute(
                &mut doc,
                Command::SetCell {
                    row: 2,
                    column: "tags".into(),
                    value: Some("[1, oops]".into()),
                },
            );
            assert!(bad.unwrap_err().to_string().contains("JSON"));

            execute(
                &mut doc,
                Command::ExpandStruct {
                    column: "point".into(),
                    all_levels: false,
                },
            )
            .unwrap();
            assert_eq!(cell(&doc, 2, "point.x").as_deref(), Some("10"));
            execute(
                &mut doc,
                Command::ExplodeList {
                    column: "tags".into(),
                },
            )
            .unwrap();
            assert_eq!(doc.status(), &Status::Computing { step: 2 });
            doc.compute_all().unwrap();
            assert_eq!(doc.num_rows(), 7);
            assert_eq!(cell(&doc, 1, "tags").as_deref(), Some("b"));
            assert_eq!(cell(&doc, 1, "point.y").as_deref(), Some("2"));
            assert!(
                execute(
                    &mut doc,
                    Command::ExpandStruct {
                        column: "tags".into(),
                        all_levels: false,
                    }
                )
                .is_err()
            );
            // All levels: owner → owner.name, owner.home → owner.home.x, ….
            execute(
                &mut doc,
                Command::ExpandStruct {
                    column: "owner".into(),
                    all_levels: true,
                },
            )
            .unwrap();
            let schema = doc.schema();
            assert!(schema.field_with_name("owner.home.x").is_ok());
            assert!(schema.field_with_name("owner.home").is_err());
            assert_eq!(cell(&doc, 0, "owner.home.x").as_deref(), Some("5"));
            let out = dir.join("out.parquet");
            doc.save(Some(out.clone())).unwrap();
            assert_eq!(
                Document::open(&out, OpenOptions::default())
                    .unwrap()
                    .num_rows(),
                7
            );
        }
    }

    #[test]
    fn collect_values_into_lists() {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);
        for options in [OpenOptions::default(), OpenOptions::paged()] {
            let mut doc = Document::open(&path, options).unwrap();
            let collect = |keys: &[&str], columns: &[&str]| Command::CollectList {
                keys: keys.iter().map(|c| c.to_string()).collect(),
                columns: columns.iter().map(|c| c.to_string()).collect(),
            };
            execute(&mut doc, collect(&["name"], &["score", "id"])).unwrap();
            doc.compute_all().unwrap();
            let names: Vec<_> = doc
                .schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect();
            assert_eq!(names, ["id", "name", "score"], "input order, flag dropped");
            assert_eq!(doc.num_rows(), 6, "five names and null");
            assert_eq!(cell(&doc, 0, "name").as_deref(), Some("ana"));
            assert!(
                cell(&doc, 0, "id")
                    .unwrap()
                    .starts_with("[5, 10, 15, 20, 25, 30, 40")
            );
            assert_eq!(cell(&doc, 5, "name"), None, "nulls group last");
            let batch = doc.read(0..6).unwrap();
            let items: usize = (0..6)
                .map(|r| crate::display::item_count(batch.column(0).as_ref(), r))
                .sum();
            assert_eq!(items, fixtures::MIXED_SETTINGS_ROWS);
            assert_eq!(
                doc.steps()[0].describe(),
                "Collected 2 columns into lists by name"
            );

            assert!(execute(&mut doc, collect(&["name"], &[])).is_err());
            assert!(execute(&mut doc, collect(&["name"], &["name"])).is_err());
        }

        // Without keys every row is one group, across reads.
        let path = dir.join("large.parquet");
        let rows = 64 * 1024 + 10; // more than one read of a full pass
        fixtures::large(&path, rows, 50_000);
        let mut doc = Document::open(&path, OpenOptions::paged()).unwrap();
        execute(
            &mut doc,
            Command::CollectList {
                keys: Vec::new(),
                columns: vec!["id".into()],
            },
        )
        .unwrap();
        doc.compute_all().unwrap();
        assert_eq!(doc.num_rows(), 1);
        let batch = doc.read(0..1).unwrap();
        assert_eq!(
            crate::display::item_count(batch.column(0).as_ref(), 0),
            rows
        );
    }

    #[test]
    fn new_nested_columns_save() {
        use arrow::datatypes::{DataType, Field};
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let point = DataType::Struct(
            vec![
                Field::new("x", DataType::Float64, true),
                Field::new("y", DataType::Utf8, true),
            ]
            .into(),
        );
        for (name, data_type) in [
            ("point", point),
            ("tags", DataType::new_list(DataType::Int64, true)),
        ] {
            execute(
                &mut doc,
                Command::AddColumn {
                    name: name.into(),
                    data_type,
                    at: 0,
                },
            )
            .unwrap();
        }
        execute(&mut doc, set_cell(1, "point", Some(r#"{"x": 1.5}"#))).unwrap();
        execute(&mut doc, set_cell(2, "tags", Some("[1, 2]"))).unwrap();
        let out = dir.join("nested-new.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(
            cell(&saved, 1, "point").as_deref(),
            Some("{x: 1.5, y: null}")
        );
        assert_eq!(cell(&saved, 2, "tags").as_deref(), Some("[1, 2]"));
        assert_eq!(cell(&saved, 0, "point"), None);
    }

    #[test]
    fn combine_columns_into_struct() {
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        for options in [OpenOptions::default(), OpenOptions::paged()] {
            let mut doc = Document::open(&path, options).unwrap();
            let combine = |columns: &[&str], name: &str| Command::CombineStruct {
                columns: columns.iter().map(|c| c.to_string()).collect(),
                name: name.into(),
            };
            // Reverses Expand: `point.` is dropped from the field names.
            execute(
                &mut doc,
                Command::ExpandStruct {
                    column: "point".into(),
                    all_levels: false,
                },
            )
            .unwrap();
            execute(&mut doc, combine(&["point.x", "point.y"], "point")).unwrap();
            assert_eq!(doc.schema().index_of("point").unwrap(), 1);
            assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 1, y: 2}"));
            assert_eq!(
                cell(&doc, 2, "point").as_deref(),
                Some("{x: null, y: null}")
            );

            execute(&mut doc, combine(&["tags", "id"], "rec")).unwrap();
            let schema = doc.schema();
            assert_eq!(schema.field(0).name(), "rec", "where the first column was");
            assert_eq!(
                cell(&doc, 0, "rec").as_deref(),
                Some("{tags: [a, b], id: 0}")
            );
            assert_eq!(doc.steps()[2].describe(), "Combined 2 columns into rec");

            assert!(
                execute(&mut doc, combine(&["path"], "owner")).is_err(),
                "taken"
            );
            assert!(execute(&mut doc, combine(&["path", "path"], "p")).is_err());
            assert!(execute(&mut doc, combine(&[], "p")).is_err());
            assert!(execute(&mut doc, combine(&["nope"], "p")).is_err());

            let out = dir.join("combined.parquet");
            doc.save(Some(out.clone())).unwrap();
            let saved = Document::open(&out, OpenOptions::default()).unwrap();
            assert_eq!(
                cell(&saved, 3, "rec").as_deref(),
                Some("{tags: [c, null, d], id: 3}")
            );
        }
    }

    #[test]
    fn failed_step_is_not_recorded() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let err = execute(&mut doc, Command::InsertRows { at: 5000, count: 1 });
        assert!(err.is_err());
        assert!(!doc.history().can_undo());
    }

    #[test]
    fn blank_document_takes_rows_and_columns() {
        let mut doc = Document::new();
        execute(&mut doc, Command::InsertRows { at: 0, count: 3 }).unwrap();
        assert_eq!(doc.num_rows(), 3);
        execute(
            &mut doc,
            Command::AddColumn {
                name: "a".into(),
                data_type: DataType::Int64,
                at: 0,
            },
        )
        .unwrap();
        assert_eq!((doc.num_rows(), doc.num_columns()), (3, 1));
        execute(
            &mut doc,
            Command::SetCell {
                row: 1,
                column: "a".into(),
                value: Some("7".into()),
            },
        )
        .unwrap();
        let dir = TempDir::new();
        doc.save(Some(dir.join("new.parquet"))).unwrap();
        let back = Document::open(dir.join("new.parquet"), OpenOptions::default()).unwrap();
        assert_eq!((back.num_rows(), back.num_columns()), (3, 1));
    }

    #[test]
    fn column_metadata_is_saved_and_follows_renames() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
        let entries = vec![KeyValue {
            key: "unit".into(),
            value: Some("m".into()),
        }];
        let set = |column: &str| Command::ReplaceColumnMetadata {
            column: column.into(),
            entries: entries.clone(),
        };
        assert!(execute(&mut doc, set("missing")).is_err());
        execute(&mut doc, set("score")).unwrap();
        execute(
            &mut doc,
            Command::RenameColumn {
                from: "score".into(),
                to: "points".into(),
            },
        )
        .unwrap();
        assert_eq!(doc.metadata().column("points"), Some(entries.as_slice()));
        let out = dir.join("out.parquet");
        doc.save(Some(out.clone())).unwrap();
        let back = Document::open(&out, OpenOptions::default()).unwrap();
        let schema = back.schema();
        let field = schema.field_with_name("points").unwrap();
        assert_eq!(field.metadata().get("unit").map(String::as_str), Some("m"));
        assert!(schema.field_with_name("id").unwrap().metadata().is_empty());
        assert!(undo(&mut doc));
        assert_eq!(doc.metadata().column("score"), Some(entries.as_slice()));
    }

    #[test]
    fn leaf_columns_of_nested_schema() {
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        let doc = Document::open(&path, OpenOptions::default()).unwrap();
        let leaves = crate::io::leaf_columns(&doc.schema()).unwrap();
        let names: Vec<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
        assert!(names.contains(&"point.x"), "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("tags.list.")),
            "{names:?}"
        );
        assert!(names.contains(&"owner.home.x"), "{names:?}");
        let (_, x) = leaves.iter().find(|(p, _)| p == "point.x").unwrap();
        assert_eq!(x, &DataType::Int64);
    }

    #[test]
    fn failing_cast_breaks_its_step() {
        use crate::steps::{Cast, CastOptions, OnError, Status};
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        // "name" holds text like "ana": not a number.
        execute(
            &mut doc,
            Command::Cast(Cast {
                column: "name".into(),
                to: DataType::Int64,
                options: CastOptions {
                    on_error: OnError::Fail,
                    ..CastOptions::default()
                },
            }),
        )
        .unwrap();
        assert!(doc.compute_all().is_err());
        let Status::Broken { step, error } = doc.status() else {
            panic!("expected a broken step, got {:?}", doc.status());
        };
        assert_eq!(*step, 0);
        assert!(error.contains("ow 2: \"bo\" in column name"), "{error}");
        // A cast that works is computed and read normally.
        assert!(undo(&mut doc));
        execute(
            &mut doc,
            Command::Cast(Cast {
                column: "id".into(),
                to: DataType::Utf8,
                options: CastOptions {
                    on_error: OnError::Fail,
                    ..CastOptions::default()
                },
            }),
        )
        .unwrap();
        doc.compute_all().unwrap();
        assert!(matches!(doc.status(), Status::Ready));
        assert_eq!(cell(&doc, 0, "id").as_deref(), Some("0"));
    }

    #[test]
    fn paste_cells_is_one_change_and_adds_rows() {
        let dir = TempDir::new();
        let mut doc = mixed(&dir);
        let values = vec![
            vec![Some("x".into()), Some("1,5".into())],
            vec![None, Some("2.5".into())],
        ];
        let paste = |row| Command::PasteCells {
            row,
            column: 1,
            values: values.clone(),
        };
        // "1,5" isn't a float64: nothing changes.
        assert!(execute(&mut doc, paste(0)).is_err());
        assert!(!doc.history().can_undo());
        let values = vec![
            vec![Some("x".into()), Some("1.5".into())],
            vec![None, Some("2.5".into())],
        ];
        execute(
            &mut doc,
            Command::PasteCells {
                row: 999,
                column: 1,
                values: values.clone(),
            },
        )
        .unwrap();
        assert_eq!(doc.num_rows(), 1001);
        assert_eq!(cell(&doc, 999, "name").as_deref(), Some("x"));
        assert_eq!(cell(&doc, 1000, "score").as_deref(), Some("2.5"));
        assert_eq!(cell(&doc, 1000, "name"), None);
        assert_eq!(doc.history().undo_label(), Some("Paste 4 cells"));
        assert!(undo(&mut doc));
        assert_eq!(doc.num_rows(), 1000);
        assert!(!doc.history().can_undo());
        // Too wide for the columns to the right.
        assert!(
            execute(
                &mut doc,
                Command::PasteCells {
                    row: 0,
                    column: 3,
                    values,
                }
            )
            .is_err()
        );
    }
}
