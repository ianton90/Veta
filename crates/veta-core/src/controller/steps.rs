//! Transformation steps (filter, sort, cast, …) and managing the steps list: remove, move, replace, append.

use super::*;

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<Change> {
    match command {
        Command::Filter(filter) => Ok(Change::PushStep(Step::Filter(filter))),
        Command::Sort(sort) => Ok(Change::PushStep(Step::Sort(sort))),
        Command::Cast(cast) => Ok(Change::PushStep(Step::Cast(cast))),
        Command::Replace(replace) => Ok(Change::PushStep(Step::Replace(replace))),
        Command::FormatText { columns, op } => {
            Ok(Change::PushStep(Step::FormatText { columns, op }))
        }
        Command::Fill(fill) => Ok(Change::PushStep(Step::Fill(fill))),
        Command::Split(split) => Ok(Change::PushStep(Step::Split(split))),
        Command::ExpandStruct { column, all_levels } => {
            Ok(Change::PushStep(Step::ExpandStruct { column, all_levels }))
        }
        Command::ExplodeList { column } => Ok(Change::PushStep(Step::ExplodeList { column })),
        Command::CollectList { keys, columns } => {
            Ok(Change::PushStep(Step::CollectList { keys, columns }))
        }
        Command::CombineStruct { columns, name } => {
            Ok(Change::PushStep(Step::CombineStruct { columns, name }))
        }
        Command::Merge(merge) => Ok(Change::PushStep(Step::Merge(merge))),
        Command::CustomColumn(custom) => Ok(Change::PushStep(Step::CustomColumn(custom))),
        Command::RemoveStep { index } => {
            let mut after = doc.steps().to_vec();
            if index >= after.len() {
                return Err(invalid(format!("there is no step {}", index + 1)));
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
                return Err(invalid("step position out of range".into()));
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
                return Err(invalid("there are no steps to use".into()));
            }
            Ok(Change::Steps {
                before: doc.steps().to_vec(),
                after: steps,
                keep: 0,
            })
        }
        Command::AppendSteps { steps } => {
            if steps.is_empty() {
                return Err(invalid("there are no steps to add".into()));
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
                return Err(invalid(format!("there is no step {}", index + 1)));
            };
            *slot = step;
            Ok(Change::Steps {
                before: doc.steps().to_vec(),
                after,
                keep: index,
            })
        }
        other => Err(invalid(format!("not a {} command: {other:?}", "steps"))),
    }
}
