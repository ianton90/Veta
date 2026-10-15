//! Column commands: add, remove, keep, rename and move columns, and add or remove struct fields.

use super::*;

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<(String, Change)> {
    match command {
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
        Command::AddField {
            column,
            name,
            data_type,
        } => {
            let step = Step::AddField {
                column,
                name,
                data_type,
            };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::RemoveField { column, name } => {
            let step = Step::RemoveField { column, name };
            Ok((step.describe(), Change::PushStep(step)))
        }
        other => Err(invalid(format!("not a {} command: {other:?}", "columns"))),
    }
}
