//! The controller: the only code that mutates a [`Document`].
//!
//! Every command is validated and turned into a reversible change, applied,
//! and recorded in the document's undo history.

mod cells;
mod columns;
pub mod command;
mod metadata;
mod nested;
pub mod rows;
mod steps;

use crate::command::Command;
use crate::error::Result;
use crate::error::invalid;
use crate::history::Change;
use crate::model::{Document, KeyValue};
use crate::steps::{OnError, Status, Step};
use crate::values::parse_value;

/// Validates `command` and applies it to `document`, recording it for undo.
///
/// On error the document is left unchanged.
pub fn execute(document: &mut Document, command: Command) -> Result<()> {
    let checked =
        matches!(&command, Command::Cast(c) if c.options.on_error == OnError::ErrorColumn);
    let change = plan(document, command)?;
    change.apply(document)?;
    document.bump_revision();
    document.history.record(change);
    if checked {
        document.error_check = document.steps().len().checked_sub(1);
    }
    Ok(())
}

/// A new cast's error column, kept because some values failed to convert
/// (see [`settle`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorsKept {
    pub column: String,
    /// How many values failed, as the check counted them.
    pub values: Option<String>,
}

/// Call when a document's full pass finishes (after
/// [`Document::install`] or [`Document::fail_compute`]). A cast added with
/// an error column is followed by a step removing that column if it is
/// empty; when that check fails, some values failed, so the check is
/// dropped (undone with the cast) and the column kept.
pub fn settle(document: &mut Document) -> Option<ErrorsKept> {
    let index = document.error_check?;
    let column = match document.steps().get(index) {
        Some(Step::RemoveEmptyColumn { name }) => name.clone(),
        _ => {
            document.error_check = None;
            return None;
        }
    };
    match document.status().clone() {
        Status::Broken { step, error } if step == index => {
            document.error_check = None;
            amend(document, Command::RemoveStep { index }).ok()?;
            // The check's error says how many values failed.
            let values = match error.as_ref() {
                crate::Error::Invalid(invalid) => invalid.arg("values").map(str::to_owned),
                _ => None,
            };
            Some(ErrorsKept { column, values })
        }
        Status::Ready => {
            document.error_check = None;
            None
        }
        _ => None,
    }
}

/// Like [`execute`], but folds the change into the last one in the
/// history, so they undo as one (for follow-up changes the app makes on its
/// own).
pub fn amend(document: &mut Document, command: Command) -> Result<()> {
    let change = plan(document, command)?;
    change.apply(document)?;
    document.bump_revision();
    document.history.amend(change);
    Ok(())
}

/// Reverts the last change. Returns `false` if there was nothing to undo.
pub fn undo(document: &mut Document) -> bool {
    let Some(change) = document.history.pop_undo() else {
        return false;
    };
    change.revert(document);
    document.bump_revision();
    true
}

/// Re-applies the last undone change. Returns `Ok(false)` if there was
/// nothing to redo.
pub fn redo(document: &mut Document) -> Result<bool> {
    let Some(change) = document.history.pop_redo() else {
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
fn plan(doc: &Document, command: Command) -> Result<Change> {
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
        command @ (C::SetValue { .. }
        | C::InsertItem { .. }
        | C::RemoveItem { .. }
        | C::SetKey { .. }) => nested::plan(doc, command),
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
            return Err(invalid!(MetadataKeyEmpty));
        }
        if entries[..i].iter().any(|other| other.key == kv.key) {
            return Err(invalid!(MetadataKeyTwice, key = kv.key));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
