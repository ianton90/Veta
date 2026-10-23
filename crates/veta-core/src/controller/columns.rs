//! Renaming a column, which also renames its writer settings and edited
//! metadata.

use super::*;

/// The change renaming column `from` to `to`.
pub(super) fn rename(doc: &Document, from: String, to: String) -> Change {
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
    if changes.len() == 1 {
        changes.remove(0)
    } else {
        Change::Batch(changes)
    }
}
