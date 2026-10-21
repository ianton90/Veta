//! English names for the model's steps and options, as the CLI prints them.
//! The core has no wording of its own (decision 53); the GUI has its own,
//! translated, names.

use veta_core::display::type_name;
use veta_core::steps::{Condition, Filter, FilterOp, SplitBy, Step, TextOp, type_path};

/// A step, e.g. "Sorted by score descending".
pub fn step(step: &Step) -> String {
    let plural = |n: usize, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    match step {
        Step::EditCells(edits) => format!("Edited {}", plural(edits.len(), "cell")),
        Step::InsertRows { count, .. } => format!("Inserted {}", plural(*count, "row")),
        Step::DeleteRows { rows } => {
            let n = rows.iter().map(|r| r.len()).sum();
            format!("Deleted {}", plural(n, "row"))
        }
        Step::AddColumn {
            name, data_type, ..
        } => format!("Added column {name} ({})", type_name(data_type)),
        Step::KeepColumns { names } => match names.as_slice() {
            [one] => format!("Kept column {one}"),
            _ => format!("Kept {} columns", names.len()),
        },
        Step::RemoveColumns { names } => match names.as_slice() {
            [one] => format!("Removed column {one}"),
            _ => format!("Removed {} columns", names.len()),
        },
        Step::RenameColumn { from, to } => format!("Renamed {from} to {to}"),
        Step::MoveColumn { name, .. } => format!("Moved column {name}"),
        Step::Filter(filter) => {
            let verb = if filter.exclude { "Removed" } else { "Kept" };
            format!("{verb} rows where {}", conditions(filter))
        }
        Step::Sort(sort) => {
            let keys: Vec<String> = sort
                .keys
                .iter()
                .map(|k| {
                    let order = if k.descending {
                        "descending"
                    } else {
                        "ascending"
                    };
                    let nulls = if k.nulls_first { ", nulls first" } else { "" };
                    format!("{} {order}{nulls}", k.column)
                })
                .collect();
            format!("Sorted by {}", keys.join(", then "))
        }
        Step::Cast(cast) => format!("Changed {} to {}", cast.column, type_name(&cast.to)),
        Step::Replace(replace) => {
            let find = match &replace.find {
                Some(f) => format!("{f:?}"),
                None => "nulls".into(),
            };
            let with = match &replace.with {
                Some(w) => format!("{w:?}"),
                None => "null".into(),
            };
            format!("Replaced {find} with {with} in {}", replace.column)
        }
        Step::FormatText { columns, op } => {
            let verb = match op {
                TextOp::Trim => "Trimmed",
                TextOp::Lower => "Lowercased",
                TextOp::Upper => "Uppercased",
                TextOp::Title => "Capitalized",
            };
            match columns.as_slice() {
                [one] => format!("{verb} {one}"),
                _ => format!("{verb} {} columns", columns.len()),
            }
        }
        Step::Fill(fill) => {
            let direction = if fill.up { "up" } else { "down" };
            match fill.columns.as_slice() {
                [one] => format!("Filled {direction} {one}"),
                many => format!("Filled {direction} {} columns", many.len()),
            }
        }
        Step::Split(split) => match &split.by {
            SplitBy::Delimiter { delimiter, parts } => format!(
                "Split {} by {delimiter:?} into {parts} columns",
                split.column
            ),
            SplitBy::Positions(positions) => {
                let list: Vec<String> = positions.iter().map(ToString::to_string).collect();
                format!("Split {} at positions {}", split.column, list.join(", "))
            }
        },
        Step::Merge(merge) => format!("Merged {} into {}", merge.columns.join(", "), merge.name),
        Step::CustomColumn(custom) => {
            format!("Added column {} = {}", custom.name, custom.expression)
        }
        Step::RemoveEmptyColumn { name } => format!("Removed column {name} if empty"),
        Step::ExpandStruct {
            column,
            all_levels: false,
        } => format!("Flattened {column} into columns"),
        Step::ExpandStruct {
            column,
            all_levels: true,
        } => format!("Flattened {column} into columns (all levels)"),
        Step::ExplodeList { column } => format!("Flattened {column} into rows"),
        Step::AddField {
            column,
            path,
            name,
            data_type,
        } => format!(
            "Added field {column}{}.{name} ({})",
            type_path(path),
            type_name(data_type)
        ),
        Step::RemoveField { column, path, name } => {
            format!("Removed field {column}{}.{name}", type_path(path))
        }
        Step::CollectList { keys, columns } => {
            let what = match columns.as_slice() {
                [one] => one.clone(),
                _ => plural(columns.len(), "column"),
            };
            match keys.as_slice() {
                [] => format!("Collected {what} into lists"),
                _ => format!("Collected {what} into lists by {}", keys.join(", ")),
            }
        }
        Step::CombineStruct { columns, name } => {
            format!("Combined {} into {name}", plural(columns.len(), "column"))
        }
    }
}

/// A filter's conditions, joined by "and" or "or".
fn conditions(filter: &Filter) -> String {
    let parts: Vec<String> = filter.conditions.iter().map(condition).collect();
    parts.join(if filter.match_all { " and " } else { " or " })
}

fn condition(c: &Condition) -> String {
    let op = filter_op(c.op);
    if c.op.needs_value() && c.value.contains('"') {
        // JSON values read better unescaped.
        format!("{} {op} {}", c.column, c.value)
    } else if c.op.needs_value() {
        format!("{} {op} {:?}", c.column, c.value)
    } else {
        format!("{} {op}", c.column)
    }
}

fn filter_op(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Equals => "equals",
        FilterOp::NotEquals => "does not equal",
        FilterOp::Less => "is less than",
        FilterOp::LessOrEqual => "is less than or equal to",
        FilterOp::Greater => "is greater than",
        FilterOp::GreaterOrEqual => "is greater than or equal to",
        FilterOp::Contains => "contains",
        FilterOp::NotContains => "does not contain",
        FilterOp::StartsWith => "starts with",
        FilterOp::EndsWith => "ends with",
        FilterOp::IsNull => "is null",
        FilterOp::IsNotNull => "is not null",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_core::steps::{Sort, SortKey};

    #[test]
    fn names_steps() {
        let sort = Step::Sort(Sort {
            keys: vec![SortKey {
                column: "score".into(),
                descending: true,
                nulls_first: true,
            }],
        });
        assert_eq!(step(&sort), "Sorted by score descending, nulls first");
        let filter = Step::Filter(Filter {
            conditions: vec![Condition {
                column: "a".into(),
                op: FilterOp::GreaterOrEqual,
                value: "3".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        });
        assert_eq!(
            step(&filter),
            "Kept rows where a is greater than or equal to \"3\""
        );
    }
}
