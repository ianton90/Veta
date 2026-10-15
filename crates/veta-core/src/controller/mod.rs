//! The controller: the only code that mutates a [`Document`].
//!
//! Every command is validated and turned into a reversible change, applied,
//! and recorded in the document's undo history.

mod cells;
mod columns;
pub mod command;
mod metadata;
pub mod rows;
mod steps;

use crate::command::Command;
use crate::error::{Error, Result};
use crate::history::Change;
use crate::model::{Document, KeyValue};
use crate::steps::Step;
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
    use Command as C;
    match command {
        command @ (C::SetMetadata { .. }
        | C::RemoveMetadata { .. }
        | C::ReplaceColumnMetadata { .. }
        | C::ReplaceMetadata(..)
        | C::SetWriterSettings(..)) => metadata::plan(doc, command),
        command @ (C::SetCell { .. } | C::PasteCells { .. } | C::PasteFields { .. }) => {
            cells::plan(doc, command)
        }
        command @ (C::InsertRows { .. } | C::DeleteRows { .. }) => rows::plan(command),
        command @ (C::AddColumn { .. }
        | C::RemoveColumns { .. }
        | C::KeepColumns { .. }
        | C::RenameColumn { .. }
        | C::MoveColumn { .. }
        | C::AddField { .. }
        | C::RemoveField { .. }) => columns::plan(doc, command),
        command => steps::plan(doc, command),
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

fn rows_label(n: usize) -> String {
    if n == 1 {
        "1 row".into()
    } else {
        format!("{n} rows")
    }
}

#[cfg(test)]
mod tests;
