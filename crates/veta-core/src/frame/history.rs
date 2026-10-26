//! Undo and redo.

use super::DataFrame;
use crate::error::{Result, invalid};
use crate::plan::History;

impl DataFrame {
    /// This frame with history on or off. With it on, every change can be
    /// undone; turning it off forgets what could be undone or redone.
    pub fn with_history(&self, on: bool) -> Self {
        Self {
            history: match on {
                true => Some(self.history.clone().unwrap_or_default()),
                false => None,
            },
            ..self.clone()
        }
    }

    /// Whether changes can be undone.
    pub fn has_history(&self) -> bool {
        self.history.is_some()
    }

    pub fn can_undo(&self) -> bool {
        self.history.as_ref().is_some_and(History::can_undo)
    }

    pub fn can_redo(&self) -> bool {
        self.history.as_ref().is_some_and(History::can_redo)
    }

    /// The frame as it was before the last change.
    pub fn undo(&self) -> Result<Self> {
        let history = self.history.as_ref();
        let (node, history) = history
            .and_then(|h| h.undo(self.node.clone()))
            .ok_or_else(|| invalid!(NothingToUndo))?;
        Ok(Self {
            node,
            history: Some(history),
            ..self.clone()
        })
    }

    /// The frame as it was before the last undo.
    pub fn redo(&self) -> Result<Self> {
        let history = self.history.as_ref();
        let (node, history) = history
            .and_then(|h| h.redo(self.node.clone()))
            .ok_or_else(|| invalid!(NothingToRedo))?;
        Ok(Self {
            node,
            history: Some(history),
            ..self.clone()
        })
    }
}
