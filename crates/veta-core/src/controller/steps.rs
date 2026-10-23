//! Adding steps, and managing the steps list: remove, move, replace, append.

use super::*;
use crate::error::invalid;
use crate::steps::normalize_ranges;

/// Plans adding `step` at the end.
pub(super) fn add(doc: &Document, step: Step) -> Result<Change> {
    match step {
        Step::DeleteRows { rows } => Ok(Change::PushStep(Step::DeleteRows {
            rows: normalize_ranges(rows),
        })),
        Step::RenameColumn { from, to } => Ok(super::columns::rename(doc, from, to)),
        // With an error column, a second step removes it if nothing failed
        // (checked on every row; see `settle`).
        Step::Cast(cast) if cast.options.on_error == OnError::ErrorColumn => {
            let name = cast.error_column();
            let before = doc.steps().to_vec();
            let keep = before.len();
            let mut after = before.clone();
            after.extend([Step::Cast(cast), Step::RemoveEmptyColumn { name }]);
            Ok(Change::Steps {
                before,
                after,
                keep,
            })
        }
        step => Ok(Change::PushStep(step)),
    }
}

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<Change> {
    match command {
        Command::RemoveStep { index } => {
            let mut after = doc.steps().to_vec();
            if index >= after.len() {
                return Err(invalid!(NoStep, step = index + 1));
            }
            after.remove(index);
            Ok(Change::Steps {
                before: doc.steps().to_vec(),
                after,
                keep: index,
            })
        }
        Command::MoveStep { from, to } => {
            let mut after = doc.steps().to_vec();
            if from >= after.len() || to >= after.len() {
                return Err(invalid!(StepPosition));
            }
            let step = after.remove(from);
            after.insert(to, step);
            Ok(Change::Steps {
                before: doc.steps().to_vec(),
                after,
                keep: from.min(to),
            })
        }
        Command::ReplaceSteps { steps } => {
            if steps.is_empty() {
                return Err(invalid!(NoStepsToUse));
            }
            Ok(Change::Steps {
                before: doc.steps().to_vec(),
                after: steps,
                keep: 0,
            })
        }
        Command::AppendSteps { steps } => {
            if steps.is_empty() {
                return Err(invalid!(NoStepsToAdd));
            }
            let before = doc.steps().to_vec();
            let keep = before.len();
            let mut after = before.clone();
            after.extend(steps);
            Ok(Change::Steps {
                before,
                after,
                keep,
            })
        }
        Command::ReplaceStep { index, step } => {
            let mut after = doc.steps().to_vec();
            let Some(slot) = after.get_mut(index) else {
                return Err(invalid!(NoStep, step = index + 1));
            };
            *slot = step;
            Ok(Change::Steps {
                before: doc.steps().to_vec(),
                after,
                keep: index,
            })
        }
        other => Err(invalid!(
            WrongArea,
            area = "steps",
            command = format!("{other:?}")
        )),
    }
}
