//! Editing, moving and removing steps. The steps after the one changed are
//! planned again on the new input; the frame before keeps its nodes, so the
//! change is undone at once.

use std::sync::Arc;

use super::DataFrame;
use crate::error::{Result, invalid};
use crate::plan::{Node, Step};

impl DataFrame {
    /// Steps, the source included.
    pub fn num_steps(&self) -> usize {
        self.node.chain().len()
    }

    /// Without step `step` (counted from 0, the first source).
    pub fn remove_step(&self, step: usize) -> Result<Self> {
        let chain = self.chain_through(step)?;
        let steps: Vec<_> = chain[step + 1..]
            .iter()
            .filter_map(|n| n.step().cloned())
            .collect();
        let base = self.base_of(&chain, step);
        self.replayed(base, &steps, step)
    }

    /// With step `from` moved to position `to`.
    pub fn move_step(&self, from: usize, to: usize) -> Result<Self> {
        let chain = self.chain_through(from.max(to))?;
        let first = from.min(to);
        let mut steps: Vec<_> = chain[first..]
            .iter()
            .filter_map(|n| n.step().cloned())
            .collect();
        let moved = steps.remove(from - first);
        steps.insert(to - first, moved);
        let base = self.base_of(&chain, first);
        self.replayed(base, &steps, first)
    }

    /// With step `step` replaced by what `edit` adds to the frame before it
    /// (any number of steps, none to remove it):
    /// `df.edit_step(2, |before| before.keep_rows(0..100))`.
    pub fn edit_step(
        &self,
        step: usize,
        edit: impl FnOnce(&DataFrame) -> Result<DataFrame>,
    ) -> Result<Self> {
        let chain = self.chain_through(step)?;
        let base = self.base_of(&chain, step);
        let before = Self {
            node: base.clone(),
            history: None,
            ..self.clone()
        };
        let edited = edit(&before)?;
        let mut node = Some(&edited.node);
        while let Some(n) = node.filter(|n| !Arc::ptr_eq(n, &base)) {
            node = n.previous();
        }
        if node.is_none() {
            return Err(invalid!(StepEditBase));
        }
        let added = edited.node.chain().len() - chain[..step].len();
        let steps: Vec<_> = chain[step + 1..]
            .iter()
            .filter_map(|n| n.step().cloned())
            .collect();
        self.replayed(edited.node.clone(), &steps, step + added)
    }

    /// The nodes from the first step's on, checking there is a step `step`.
    fn chain_through(&self, step: usize) -> Result<Vec<Arc<Node>>> {
        let chain = self.node.chain();
        if step >= chain.len() {
            return Err(invalid!(NoStep, step = step));
        }
        Ok(chain)
    }

    /// The node before step `step` of `chain`.
    fn base_of(&self, chain: &[Arc<Node>], step: usize) -> Arc<Node> {
        match step {
            0 => self.node.root(),
            _ => chain[step - 1].clone(),
        }
    }

    /// This frame changed to `steps` added to `base`, the first of them
    /// numbered `first`.
    fn replayed(&self, base: Arc<Node>, steps: &[Arc<dyn Step>], first: usize) -> Result<Self> {
        let context = self.context();
        let mut node = base;
        for (i, step) in steps.iter().enumerate() {
            node = node
                .push(step.clone(), &context)
                .map_err(|e| invalid!(StepHasError, step = first + i; cause = e))?;
        }
        Ok(self.changed_to(node))
    }
}
