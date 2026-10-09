//! Undo/redo history.
//!
//! The controller turns each command into a [`Change`] that knows how to
//! apply and revert itself. Applied changes go on the undo stack; undoing
//! moves them to the redo stack.

use arrow::array::ArrayRef;

use crate::error::{Error, Result};
use crate::model::{Document, FileMetadata, WriterSettings};
use crate::steps::{CellEdits, Step};

/// A reversible change to a document.
#[derive(Debug, Clone)]
pub(crate) enum Change {
    /// Appends a step.
    PushStep(Step),
    /// Sets one cell. When `new_step` is true the edit starts a new
    /// edit-cells step; otherwise it goes into the last step, replacing
    /// `before` (the value this cell had in that step, if any).
    EditCell {
        column: String,
        row: usize,
        before: Option<ArrayRef>,
        after: ArrayRef,
        new_step: bool,
    },
    Metadata {
        before: FileMetadata,
        after: FileMetadata,
    },
    Writer {
        before: WriterSettings,
        after: WriterSettings,
    },
    /// Several changes applied in order and reverted in reverse order.
    Batch(Vec<Change>),
}

impl Change {
    pub(crate) fn apply(&self, doc: &mut Document) -> Result<()> {
        match self {
            Change::PushStep(step) => doc.pipeline.push(step.clone()),
            Change::EditCell {
                column,
                row,
                after,
                new_step,
                ..
            } => {
                if *new_step {
                    let mut edits = CellEdits::default();
                    edits.set(column, *row, after.clone());
                    doc.pipeline.push(Step::EditCells(edits))
                } else {
                    last_edits(doc)?.set(column, *row, after.clone());
                    Ok(())
                }
            }
            Change::Metadata { after, .. } => {
                doc.metadata = after.clone();
                Ok(())
            }
            Change::Writer { after, .. } => {
                doc.writer = after.clone();
                Ok(())
            }
            Change::Batch(changes) => {
                for (i, change) in changes.iter().enumerate() {
                    if let Err(e) = change.apply(doc) {
                        // Leave the document as it was.
                        for done in changes[..i].iter().rev() {
                            done.revert(doc);
                        }
                        return Err(e);
                    }
                }
                Ok(())
            }
        }
    }

    /// Reverts a change previously applied with [`Change::apply`].
    pub(crate) fn revert(&self, doc: &mut Document) {
        match self {
            Change::PushStep(_) => {
                doc.pipeline.pop();
            }
            Change::EditCell {
                column,
                row,
                before,
                new_step,
                ..
            } => {
                if *new_step {
                    doc.pipeline.pop();
                } else if let Ok(edits) = last_edits(doc) {
                    match before {
                        Some(value) => {
                            edits.set(column, *row, value.clone());
                        }
                        None => {
                            edits.remove(column, *row);
                        }
                    }
                }
            }
            Change::Metadata { before, .. } => doc.metadata = before.clone(),
            Change::Writer { before, .. } => doc.writer = before.clone(),
            Change::Batch(changes) => {
                for change in changes.iter().rev() {
                    change.revert(doc);
                }
            }
        }
    }
}

fn last_edits(doc: &mut Document) -> Result<&mut CellEdits> {
    doc.pipeline
        .last_edits_mut()
        .ok_or_else(|| Error::InvalidCommand("the last step is not a cell edit".into()))
}

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    change: Change,
}

/// Undo and redo stacks, plus the point at which the document was saved.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// Length of `undo` when the document was last saved; `None` once that
    /// state can no longer be reached.
    saved: Option<usize>,
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            saved: Some(0),
            ..Self::default()
        }
    }

    pub(crate) fn record(&mut self, label: String, change: Change) {
        if self.saved.is_some_and(|s| s > self.undo.len()) {
            // The saved state was in the redo stack we are discarding.
            self.saved = None;
        }
        self.redo.clear();
        self.undo.push(Entry { label, change });
    }

    pub(crate) fn pop_undo(&mut self) -> Option<(String, Change)> {
        let entry = self.undo.pop()?;
        let result = (entry.label.clone(), entry.change.clone());
        self.redo.push(entry);
        Some(result)
    }

    pub(crate) fn pop_redo(&mut self) -> Option<(String, Change)> {
        let entry = self.redo.pop()?;
        let result = (entry.label.clone(), entry.change.clone());
        self.undo.push(entry);
        Some(result)
    }

    /// Moves the entry [`Self::pop_redo`] just returned back to the redo
    /// stack, after re-applying it failed.
    pub(crate) fn cancel_redo(&mut self) {
        if let Some(entry) = self.undo.pop() {
            self.redo.push(entry);
        }
    }

    pub(crate) fn mark_saved(&mut self) {
        self.saved = Some(self.undo.len());
    }

    /// Whether the document differs from what was last saved (or opened).
    pub fn is_modified(&self) -> bool {
        self.saved != Some(self.undo.len())
    }

    /// Label of the change that undo would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    /// Label of the change that redo would re-apply.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
