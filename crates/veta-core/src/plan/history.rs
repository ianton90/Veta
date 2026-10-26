//! Undo and redo: the frames' earlier and later last nodes, as two stacks
//! shared between frames (pushing makes a new top that points to the rest).
//! A node keeps its outputs, so going back to it is instant.

use std::sync::Arc;

use super::node::Node;

/// Earlier last nodes to undo to, later ones to redo to.
#[derive(Debug, Clone, Default)]
pub(crate) struct History {
    undo: Stack,
    redo: Stack,
}

#[derive(Debug, Clone, Default)]
struct Stack(Option<Arc<Entry>>);

#[derive(Debug)]
struct Entry {
    node: Arc<Node>,
    below: Stack,
}

impl Stack {
    fn push(&self, node: Arc<Node>) -> Self {
        Self(Some(Arc::new(Entry {
            node,
            below: self.clone(),
        })))
    }

    fn pop(&self) -> Option<(Arc<Node>, Self)> {
        self.0.as_ref().map(|e| (e.node.clone(), e.below.clone()))
    }
}

impl History {
    /// After a change from `node`: it can be undone to, and nothing can be
    /// redone.
    pub(crate) fn changed(&self, node: Arc<Node>) -> Self {
        Self {
            undo: self.undo.push(node),
            redo: Stack::default(),
        }
    }

    /// The node to undo to from `current`, and the history after it.
    pub(crate) fn undo(&self, current: Arc<Node>) -> Option<(Arc<Node>, Self)> {
        let (node, undo) = self.undo.pop()?;
        Some((
            node,
            Self {
                undo,
                redo: self.redo.push(current),
            },
        ))
    }

    /// The node to redo to from `current`, and the history after it.
    pub(crate) fn redo(&self, current: Arc<Node>) -> Option<(Arc<Node>, Self)> {
        let (node, redo) = self.redo.pop()?;
        Some((
            node,
            Self {
                undo: self.undo.push(current),
                redo,
            },
        ))
    }

    /// Without the undo entries at the top that `drop` says to drop (cell
    /// edits grouped into one change).
    pub(crate) fn without_top(&self, drop: impl Fn(&Node) -> bool) -> Self {
        let mut undo = self.undo.clone();
        while let Some((node, below)) = undo.pop() {
            if !drop(&node) {
                break;
            }
            undo = below;
        }
        Self {
            undo,
            redo: self.redo.clone(),
        }
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.undo.0.is_some()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.redo.0.is_some()
    }
}

impl Drop for Entry {
    /// Drops long stacks one entry at a time, not by recursion.
    fn drop(&mut self) {
        let mut below = self.below.0.take();
        while let Some(entry) = below {
            below = match Arc::try_unwrap(entry) {
                Ok(mut entry) => entry.below.0.take(),
                Err(_) => None,
            };
        }
    }
}
