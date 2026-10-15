//! Transformation steps (filter, sort, cast, …) and managing the steps list: remove, move, replace, append.

use super::*;

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<(String, Change)> {
    match command {
        Command::Filter(filter) => Ok((filter.describe(), Change::PushStep(Step::Filter(filter)))),
        Command::Sort(sort) => Ok((sort.describe(), Change::PushStep(Step::Sort(sort)))),
        Command::Cast(cast) => Ok((cast.describe(), Change::PushStep(Step::Cast(cast)))),
        Command::Replace(replace) => {
            Ok((replace.describe(), Change::PushStep(Step::Replace(replace))))
        }
        Command::FormatText { columns, op } => {
            let step = Step::FormatText { columns, op };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::Fill(fill) => Ok((fill.describe(), Change::PushStep(Step::Fill(fill)))),
        Command::Split(split) => Ok((split.describe(), Change::PushStep(Step::Split(split)))),
        Command::ExpandStruct { column, all_levels } => {
            let step = Step::ExpandStruct { column, all_levels };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::ExplodeList { column } => {
            let step = Step::ExplodeList { column };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::CollectList { keys, columns } => {
            let step = Step::CollectList { keys, columns };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::CombineStruct { columns, name } => {
            let step = Step::CombineStruct { columns, name };
            Ok((step.describe(), Change::PushStep(step)))
        }
        Command::Merge(merge) => Ok((merge.describe(), Change::PushStep(Step::Merge(merge)))),
        Command::CustomColumn(custom) => Ok((
            custom.describe(),
            Change::PushStep(Step::CustomColumn(custom)),
        )),
        Command::RemoveStep { index } => {
            let mut after = doc.steps().to_vec();
            if index >= after.len() {
                return Err(invalid(format!("there is no step {}", index + 1)));
            }
            let removed = after.remove(index);
            Ok((
                format!("Remove step: {}", removed.describe()),
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after,
                    keep: index,
                },
            ))
        }
        Command::MoveStep { from, to } => {
            let mut after = doc.steps().to_vec();
            if from >= after.len() || to >= after.len() {
                return Err(invalid("step position out of range".into()));
            }
            let step = after.remove(from);
            let label = format!("Move step: {}", step.describe());
            after.insert(to, step);
            Ok((
                label,
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after,
                    keep: from.min(to),
                },
            ))
        }
        Command::ReplaceSteps { steps } => {
            if steps.is_empty() {
                return Err(invalid("there are no steps to use".into()));
            }
            let label = match steps.len() {
                1 => "Replace the steps with 1 step".to_owned(),
                n => format!("Replace the steps with {n} steps"),
            };
            Ok((
                label,
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after: steps,
                    keep: 0,
                },
            ))
        }
        Command::AppendSteps { steps } => {
            if steps.is_empty() {
                return Err(invalid("there are no steps to add".into()));
            }
            let before = doc.steps().to_vec();
            let keep = before.len();
            let mut after = before.clone();
            let label = match steps.as_slice() {
                [one] => one.describe(),
                many => format!("Add {} steps", many.len()),
            };
            after.extend(steps);
            Ok((
                label,
                Change::Steps {
                    before,
                    after,
                    keep,
                },
            ))
        }
        Command::ReplaceStep { index, step } => {
            let mut after = doc.steps().to_vec();
            let Some(slot) = after.get_mut(index) else {
                return Err(invalid(format!("there is no step {}", index + 1)));
            };
            let label = format!("Edit step: {}", step.describe());
            *slot = step;
            Ok((
                label,
                Change::Steps {
                    before: doc.steps().to_vec(),
                    after,
                    keep: index,
                },
            ))
        }
        other => Err(invalid(format!("not a {} command: {other:?}", "steps"))),
    }
}
