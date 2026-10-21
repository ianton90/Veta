//! Texts for the model's values in the app's language: step names, filter,
//! text and on-error options, value summaries, the import report and the
//! formula help. `veta-core` describes these in plain English (for the CLI
//! and recipes); the app builds its own from the same data.

use std::fmt;

use veta_core::convert::ImportReport;
use veta_core::display::type_name;
use veta_core::steps::{
    Condition, Filter, FilterOp, OnError, SplitBy, Step, TextOp, function_signatures, type_path,
};
use veta_core::values::node::{Node, NodeKind};

use super::{tr, tr_with};

/// A model enum shown by a translated label.
pub trait Label: Copy {
    fn label(self) -> String;
}

/// Shows a model enum by its translated label (pick lists need `Display`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labeled<T>(pub T);

impl<T: Label> fmt::Display for Labeled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.label())
    }
}

impl Label for FilterOp {
    fn label(self) -> String {
        tr(match self {
            Self::Equals => "ST_FILTER_OP_EQUALS",
            Self::NotEquals => "ST_FILTER_OP_NOT_EQUALS",
            Self::Less => "ST_FILTER_OP_LESS",
            Self::LessOrEqual => "ST_FILTER_OP_LESS_OR_EQUAL",
            Self::Greater => "ST_FILTER_OP_GREATER",
            Self::GreaterOrEqual => "ST_FILTER_OP_GREATER_OR_EQUAL",
            Self::Contains => "ST_FILTER_OP_CONTAINS",
            Self::NotContains => "ST_FILTER_OP_NOT_CONTAINS",
            Self::StartsWith => "ST_FILTER_OP_STARTS_WITH",
            Self::EndsWith => "ST_FILTER_OP_ENDS_WITH",
            Self::IsNull => "ST_FILTER_OP_IS_NULL",
            Self::IsNotNull => "ST_FILTER_OP_IS_NOT_NULL",
        })
    }
}

impl Label for TextOp {
    fn label(self) -> String {
        tr(match self {
            Self::Trim => "ST_TEXT_OP_TRIM",
            Self::Lower => "ST_TEXT_OP_LOWER",
            Self::Upper => "ST_TEXT_OP_UPPER",
            Self::Title => "ST_TEXT_OP_TITLE",
        })
    }
}

impl Label for OnError {
    fn label(self) -> String {
        tr(match self {
            Self::Null => "ST_ON_ERROR_NULL",
            Self::Fail => "ST_ON_ERROR_FAIL",
            Self::ErrorColumn => "ST_ON_ERROR_COLUMN",
        })
    }
}

/// One of two tags by count: `one` (for 1) or `many` (with `{n}`).
fn count(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => tr(one),
        n => tr_with(many, &[("n", &n)]),
    }
}

/// A step's name in the applied steps list.
pub fn step_name(step: &Step) -> String {
    match step {
        Step::EditCells(edits) => count(edits.len(), "ST_STEP_EDIT_CELL", "ST_STEP_EDIT_CELLS"),
        Step::InsertRows { count: n, .. } => count(*n, "ST_STEP_INSERT_ROW", "ST_STEP_INSERT_ROWS"),
        Step::DeleteRows { rows } => {
            let n = rows.iter().map(|r| r.len()).sum();
            count(n, "ST_STEP_DELETE_ROW", "ST_STEP_DELETE_ROWS")
        }
        Step::AddColumn {
            name, data_type, ..
        } => tr_with(
            "ST_STEP_ADD_COLUMN",
            &[("name", name), ("type", &type_name(data_type))],
        ),
        Step::KeepColumns { names } => match names.as_slice() {
            [one] => tr_with("ST_STEP_KEEP_COLUMN", &[("name", one)]),
            _ => tr_with("ST_STEP_KEEP_COLUMNS", &[("n", &names.len())]),
        },
        Step::RemoveColumns { names } => match names.as_slice() {
            [one] => tr_with("ST_STEP_REMOVE_COLUMN", &[("name", one)]),
            _ => tr_with("ST_STEP_REMOVE_COLUMNS", &[("n", &names.len())]),
        },
        Step::RenameColumn { from, to } => {
            tr_with("ST_STEP_RENAME_COLUMN", &[("from", from), ("to", to)])
        }
        Step::MoveColumn { name, .. } => tr_with("ST_STEP_MOVE_COLUMN", &[("name", name)]),
        Step::Filter(filter) => {
            let tag = if filter.exclude {
                "ST_STEP_FILTER_REMOVED"
            } else {
                "ST_STEP_FILTER_KEPT"
            };
            tr_with(tag, &[("conditions", &conditions(filter))])
        }
        Step::Sort(sort) => {
            let keys: Vec<String> = sort
                .keys
                .iter()
                .map(|k| {
                    let tag = match (k.descending, k.nulls_first) {
                        (false, false) => "ST_STEP_SORT_ASCENDING",
                        (true, false) => "ST_STEP_SORT_DESCENDING",
                        (false, true) => "ST_STEP_SORT_ASCENDING_NULLS_FIRST",
                        (true, true) => "ST_STEP_SORT_DESCENDING_NULLS_FIRST",
                    };
                    tr_with(tag, &[("column", &k.column)])
                })
                .collect();
            let then = format!(", {} ", tr("ST_STEP_SORT_THEN"));
            tr_with("ST_STEP_SORT", &[("keys", &keys.join(&then))])
        }
        Step::Cast(cast) => tr_with(
            "ST_STEP_CAST",
            &[("column", &cast.column), ("type", &type_name(&cast.to))],
        ),
        Step::Replace(replace) => {
            let find = match &replace.find {
                Some(f) => format!("{f:?}"),
                None => tr("ST_STEP_REPLACE_NULLS"),
            };
            let with = match &replace.with {
                Some(w) => format!("{w:?}"),
                None => "null".into(),
            };
            tr_with(
                "ST_STEP_REPLACE",
                &[
                    ("find", &find),
                    ("with", &with),
                    ("column", &replace.column),
                ],
            )
        }
        Step::FormatText { columns, op } => {
            let (one, many) = match op {
                TextOp::Trim => ("ST_STEP_TRIM_ONE", "ST_STEP_TRIM_MANY"),
                TextOp::Lower => ("ST_STEP_LOWER_ONE", "ST_STEP_LOWER_MANY"),
                TextOp::Upper => ("ST_STEP_UPPER_ONE", "ST_STEP_UPPER_MANY"),
                TextOp::Title => ("ST_STEP_TITLE_ONE", "ST_STEP_TITLE_MANY"),
            };
            match columns.as_slice() {
                [column] => tr_with(one, &[("column", column)]),
                _ => tr_with(many, &[("n", &columns.len())]),
            }
        }
        Step::Fill(fill) => match (fill.columns.as_slice(), fill.up) {
            ([one], false) => tr_with("ST_STEP_FILL_DOWN_ONE", &[("column", one)]),
            ([one], true) => tr_with("ST_STEP_FILL_UP_ONE", &[("column", one)]),
            (many, false) => tr_with("ST_STEP_FILL_DOWN_MANY", &[("n", &many.len())]),
            (many, true) => tr_with("ST_STEP_FILL_UP_MANY", &[("n", &many.len())]),
        },
        Step::Split(split) => match &split.by {
            SplitBy::Delimiter { delimiter, parts } => tr_with(
                "ST_STEP_SPLIT_DELIMITER",
                &[
                    ("column", &split.column),
                    ("delimiter", &format!("{delimiter:?}")),
                    ("n", parts),
                ],
            ),
            SplitBy::Positions(positions) => {
                let list: Vec<String> = positions.iter().map(ToString::to_string).collect();
                tr_with(
                    "ST_STEP_SPLIT_POSITIONS",
                    &[("column", &split.column), ("positions", &list.join(", "))],
                )
            }
        },
        Step::Merge(merge) => tr_with(
            "ST_STEP_MERGE",
            &[
                ("columns", &merge.columns.join(", ")),
                ("name", &merge.name),
            ],
        ),
        Step::CustomColumn(custom) => tr_with(
            "ST_STEP_CUSTOM_COLUMN",
            &[("name", &custom.name), ("formula", &custom.expression)],
        ),
        Step::RemoveEmptyColumn { name } => {
            tr_with("ST_STEP_REMOVE_EMPTY_COLUMN", &[("name", name)])
        }
        Step::ExpandStruct {
            column,
            all_levels: false,
        } => tr_with("ST_STEP_EXPAND_STRUCT", &[("column", column)]),
        Step::ExpandStruct {
            column,
            all_levels: true,
        } => tr_with("ST_STEP_EXPAND_ALL", &[("column", column)]),
        Step::ExplodeList { column } => tr_with("ST_STEP_EXPLODE_LIST", &[("column", column)]),
        Step::AddField {
            column,
            path,
            name,
            data_type,
        } => tr_with(
            "ST_STEP_ADD_FIELD",
            &[
                ("field", &format!("{column}{}.{name}", type_path(path))),
                ("type", &type_name(data_type)),
            ],
        ),
        Step::RemoveField { column, path, name } => tr_with(
            "ST_STEP_REMOVE_FIELD",
            &[("field", &format!("{column}{}.{name}", type_path(path)))],
        ),
        Step::CollectList { keys, columns } => {
            let what = match columns.as_slice() {
                [one] => one.clone(),
                _ => count(columns.len(), "ST_STEP_ONE_COLUMN", "ST_STEP_N_COLUMNS"),
            };
            match keys.as_slice() {
                [] => tr_with("ST_STEP_COLLECT", &[("what", &what)]),
                _ => tr_with(
                    "ST_STEP_COLLECT_BY",
                    &[("what", &what), ("keys", &keys.join(", "))],
                ),
            }
        }
        Step::CombineStruct { columns, name } => tr_with(
            "ST_STEP_COMBINE",
            &[
                (
                    "what",
                    &count(columns.len(), "ST_STEP_ONE_COLUMN", "ST_STEP_N_COLUMNS"),
                ),
                ("name", name),
            ],
        ),
    }
}

/// A filter's conditions, joined by "and" or "or".
pub fn conditions(filter: &Filter) -> String {
    let parts: Vec<String> = filter.conditions.iter().map(condition).collect();
    let join = tr(if filter.match_all {
        "ST_STEP_AND"
    } else {
        "ST_STEP_OR"
    });
    parts.join(&format!(" {join} "))
}

fn condition(c: &Condition) -> String {
    let op = c.op.label();
    if c.op.needs_value() && c.value.contains('"') {
        // JSON values read better unescaped.
        format!("{} {op} {}", c.column, c.value)
    } else if c.op.needs_value() {
        format!("{} {op} {:?}", c.column, c.value)
    } else {
        format!("{} {op}", c.column)
    }
}

/// A value as one line: lists and maps by their size (`[2 items]`), other
/// values as their text; `None` for null.
pub fn summary(node: &Node) -> Option<String> {
    if node.is_null() {
        return None;
    }
    Some(match (node.kind(), node.len()) {
        (NodeKind::List, 0) => "[]".into(),
        (NodeKind::List, 1) => tr("ST_VALUE_ONE_ITEM"),
        (NodeKind::List, n) => tr_with("ST_VALUE_ITEMS", &[("n", &n)]),
        (NodeKind::Map, 0) => "{}".into(),
        (NodeKind::Map, 1) => tr("ST_VALUE_ONE_ENTRY"),
        (NodeKind::Map, n) => tr_with("ST_VALUE_ENTRIES", &[("n", &n)]),
        _ => return node.text(),
    })
}

/// What an import couldn't fit, if anything.
pub fn import_report(report: &ImportReport) -> Option<String> {
    if report.failures.is_empty() {
        return None;
    }
    let total: u64 = report.failures.iter().map(|(_, n)| n).sum();
    let columns: Vec<String> = report
        .failures
        .iter()
        .map(|(c, n)| format!("{c} ({n})"))
        .collect();
    let tag = match report.on_error {
        Some(OnError::ErrorColumn) => "ST_IMPORT_REPORT_ERROR_COLUMNS",
        _ => "ST_IMPORT_REPORT_NULL",
    };
    Some(tr_with(
        tag,
        &[("n", &total), ("columns", &columns.join(", "))],
    ))
}

/// How formulas are written: columns, operators and functions.
pub fn formula_help() -> String {
    tr_with(
        "ST_FORMULA_HELP",
        &[("functions", &function_signatures().join(", "))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_core::steps::Sort;
    use veta_core::steps::SortKey;

    // English texts match what veta-core says, so recipes, the CLI and the
    // app name steps alike.
    #[test]
    fn english_matches_the_core() {
        let steps = [
            Step::Sort(Sort {
                keys: vec![SortKey {
                    column: "a".into(),
                    descending: true,
                    nulls_first: true,
                }],
            }),
            Step::Filter(Filter {
                conditions: vec![Condition {
                    column: "a".into(),
                    op: FilterOp::GreaterOrEqual,
                    value: "3".into(),
                    case_sensitive: false,
                }],
                match_all: true,
                exclude: false,
            }),
            Step::KeepColumns {
                names: vec!["a".into(), "b".into()],
            },
            Step::FormatText {
                columns: vec!["a".into()],
                op: TextOp::Upper,
            },
        ];
        for step in &steps {
            assert_eq!(step_name(step), step.describe());
        }
    }
}
