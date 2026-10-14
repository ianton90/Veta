//! Recipes: a document's steps saved as JSON, to apply to other files.
//!
//! ```json
//! {
//!   "format": "veta-recipe",
//!   "version": 1,
//!   "name": "sales",
//!   "description": "",
//!   "created_with": "veta 0.1.0",
//!   "source_file": "sales.parquet",
//!   "steps": [
//!     {
//!       "order": 1,
//!       "description": "Renamed name to label",
//!       "type": "rename_column",
//!       "settings": { "from": "name", "to": "label" }
//!     }
//!   ]
//! }
//! ```
//!
//! Steps run by `order`; `description` fields are for people and ignored
//! when loading. Types are written as in `veta-cli schema` (`int64`,
//! `timestamp[us]`, …) and edited cell values as text, read back with the
//! column's type.

use std::sync::Arc;

use arrow::datatypes::{DataType, SchemaRef};
use arrow::util::display::array_value_to_string;
use serde::{Deserialize, Serialize};

use crate::display::{parse_type_name, type_name};
use crate::error::{Error, Result};
use crate::steps::{
    Cast, CastOptions, CellEdits, Condition, CustomColumn, Fill, Filter, FilterOp, Merge, OnError,
    Replace, Sort, SortKey, Split, SplitBy, Step, TextOp, schema_after,
};
use crate::values::parse_value;

/// Marks a JSON file as a recipe.
const FORMAT: &str = "veta-recipe";
/// Version written to new recipes.
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Recipe {
    format: String,
    version: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    created_with: String,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default)]
    steps: Vec<RecipeEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecipeEntry {
    order: usize,
    /// For people reading the file; ignored when loading.
    #[serde(default)]
    description: String,
    #[serde(flatten)]
    step: RecipeStep,
}

/// What describes a recipe besides its steps.
#[derive(Debug, Clone, Default)]
pub struct RecipeInfo {
    pub name: String,
    pub description: String,
    /// File the steps were made on.
    pub source_file: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
enum RecipeStep {
    EditCells {
        edits: Vec<RecipeEdit>,
    },
    InsertRows {
        at: usize,
        count: usize,
    },
    DeleteRows {
        /// `[start, end)` row ranges, from 0.
        rows: Vec<[usize; 2]>,
    },
    AddColumn {
        name: String,
        #[serde(rename = "type")]
        data_type: String,
        at: usize,
    },
    RemoveColumns {
        names: Vec<String>,
    },
    KeepColumns {
        names: Vec<String>,
    },
    RenameColumn {
        from: String,
        to: String,
    },
    MoveColumn {
        name: String,
        to: usize,
    },
    Filter {
        match_all: bool,
        #[serde(default)]
        exclude: bool,
        conditions: Vec<RecipeCondition>,
    },
    Sort {
        keys: Vec<RecipeSortKey>,
    },
    Cast {
        column: String,
        #[serde(rename = "type")]
        data_type: String,
        #[serde(default = "dot")]
        decimal: char,
        #[serde(default)]
        thousands: Option<char>,
        #[serde(default)]
        format: Option<String>,
        #[serde(default = "null_on_error")]
        on_error: RecipeOnError,
    },
    Replace {
        column: String,
        /// Missing: find nulls.
        #[serde(default)]
        find: Option<String>,
        /// Missing: replace with null.
        #[serde(default)]
        with: Option<String>,
        #[serde(default)]
        whole_cell: bool,
    },
    FormatText {
        columns: Vec<String>,
        op: RecipeTextOp,
    },
    Fill {
        columns: Vec<String>,
        #[serde(default)]
        up: bool,
    },
    Split {
        column: String,
        #[serde(default)]
        delimiter: Option<String>,
        #[serde(default)]
        parts: Option<usize>,
        #[serde(default)]
        positions: Option<Vec<usize>>,
    },
    Merge {
        columns: Vec<String>,
        separator: String,
        name: String,
    },
    CustomColumn {
        name: String,
        expression: String,
        at: usize,
    },
    RemoveEmptyColumn {
        name: String,
    },
    ExpandStruct {
        column: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_levels: bool,
    },
    ExplodeList {
        column: String,
    },
    CombineStruct {
        columns: Vec<String>,
        name: String,
    },
    CollectList {
        keys: Vec<String>,
        columns: Vec<String>,
    },
}

fn dot() -> char {
    '.'
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeEdit {
    row: usize,
    column: String,
    /// Missing: null.
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeCondition {
    column: String,
    op: RecipeFilterOp,
    #[serde(default)]
    value: String,
    #[serde(default)]
    case_sensitive: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeSortKey {
    column: String,
    #[serde(default)]
    descending: bool,
    #[serde(default)]
    nulls_first: bool,
}

/// Mirrors an enum of the steps module with snake_case names.
macro_rules! mirror {
    ($name:ident, $target:ty, $($variant:ident),+) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $name {
            $($variant),+
        }

        impl From<$target> for $name {
            fn from(value: $target) -> Self {
                match value {
                    $(<$target>::$variant => Self::$variant),+
                }
            }
        }

        impl From<$name> for $target {
            fn from(value: $name) -> Self {
                match value {
                    $($name::$variant => Self::$variant),+
                }
            }
        }
    };
}

mirror!(
    RecipeFilterOp,
    FilterOp,
    Equals,
    NotEquals,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    IsNull,
    IsNotNull
);
mirror!(RecipeOnError, OnError, Null, Fail, ErrorColumn);
mirror!(RecipeTextOp, TextOp, Trim, Lower, Upper, Title);

fn null_on_error() -> RecipeOnError {
    RecipeOnError::Null
}

fn recipe_error(message: impl std::fmt::Display) -> Error {
    Error::InvalidCommand(format!("recipe: {message}"))
}

fn parse_type(name: &str) -> Result<DataType> {
    parse_type_name(name)
        .or_else(|| name.parse().ok())
        .ok_or_else(|| recipe_error(format!("unknown type {name:?}")))
}

impl RecipeStep {
    fn from_step(step: &Step) -> Result<Self> {
        Ok(match step {
            Step::EditCells(edits) => RecipeStep::EditCells {
                edits: edits
                    .iter()
                    .map(|(column, row, value)| {
                        let text = if value.is_null(0) {
                            None
                        } else {
                            Some(array_value_to_string(value, 0)?)
                        };
                        Ok(RecipeEdit {
                            row,
                            column: column.to_owned(),
                            value: text,
                        })
                    })
                    .collect::<Result<_>>()?,
            },
            Step::InsertRows { at, count } => RecipeStep::InsertRows {
                at: *at,
                count: *count,
            },
            Step::DeleteRows { rows } => RecipeStep::DeleteRows {
                rows: rows.iter().map(|r| [r.start, r.end]).collect(),
            },
            Step::AddColumn {
                name,
                data_type,
                at,
            } => RecipeStep::AddColumn {
                name: name.clone(),
                data_type: type_name(data_type),
                at: *at,
            },
            Step::KeepColumns { names } => RecipeStep::KeepColumns {
                names: names.clone(),
            },
            Step::RemoveColumns { names } => RecipeStep::RemoveColumns {
                names: names.clone(),
            },
            Step::RenameColumn { from, to } => RecipeStep::RenameColumn {
                from: from.clone(),
                to: to.clone(),
            },
            Step::MoveColumn { name, to } => RecipeStep::MoveColumn {
                name: name.clone(),
                to: *to,
            },
            Step::Filter(f) => RecipeStep::Filter {
                match_all: f.match_all,
                exclude: f.exclude,
                conditions: f
                    .conditions
                    .iter()
                    .map(|c| RecipeCondition {
                        column: c.column.clone(),
                        op: c.op.into(),
                        value: c.value.clone(),
                        case_sensitive: c.case_sensitive,
                    })
                    .collect(),
            },
            Step::Sort(s) => RecipeStep::Sort {
                keys: s
                    .keys
                    .iter()
                    .map(|k| RecipeSortKey {
                        column: k.column.clone(),
                        descending: k.descending,
                        nulls_first: k.nulls_first,
                    })
                    .collect(),
            },
            Step::Cast(c) => RecipeStep::Cast {
                column: c.column.clone(),
                data_type: type_name(&c.to),
                decimal: c.options.decimal,
                thousands: c.options.thousands,
                format: c.options.format.clone(),
                on_error: c.options.on_error.into(),
            },
            Step::Replace(r) => RecipeStep::Replace {
                column: r.column.clone(),
                find: r.find.clone(),
                with: r.with.clone(),
                whole_cell: r.whole_cell,
            },
            Step::FormatText { columns, op } => RecipeStep::FormatText {
                columns: columns.clone(),
                op: (*op).into(),
            },
            Step::Fill(f) => RecipeStep::Fill {
                columns: f.columns.clone(),
                up: f.up,
            },
            Step::Split(s) => match &s.by {
                SplitBy::Delimiter { delimiter, parts } => RecipeStep::Split {
                    column: s.column.clone(),
                    delimiter: Some(delimiter.clone()),
                    parts: Some(*parts),
                    positions: None,
                },
                SplitBy::Positions(positions) => RecipeStep::Split {
                    column: s.column.clone(),
                    delimiter: None,
                    parts: None,
                    positions: Some(positions.clone()),
                },
            },
            Step::Merge(m) => RecipeStep::Merge {
                columns: m.columns.clone(),
                separator: m.separator.clone(),
                name: m.name.clone(),
            },
            Step::ExpandStruct { column, all_levels } => RecipeStep::ExpandStruct {
                column: column.clone(),
                all_levels: *all_levels,
            },
            Step::CollectList { keys, columns } => RecipeStep::CollectList {
                keys: keys.clone(),
                columns: columns.clone(),
            },
            Step::CombineStruct { columns, name } => RecipeStep::CombineStruct {
                columns: columns.clone(),
                name: name.clone(),
            },
            Step::ExplodeList { column } => RecipeStep::ExplodeList {
                column: column.clone(),
            },
            Step::RemoveEmptyColumn { name } => {
                RecipeStep::RemoveEmptyColumn { name: name.clone() }
            }
            Step::CustomColumn(c) => RecipeStep::CustomColumn {
                name: c.name.clone(),
                expression: c.expression.clone(),
                at: c.at,
            },
        })
    }

    /// Converts to a step; `schema` is the step's input (for cell values).
    fn into_step(self, schema: &SchemaRef) -> Result<Step> {
        Ok(match self {
            RecipeStep::EditCells { edits } => {
                let mut cells = CellEdits::default();
                for edit in edits {
                    let field = schema.field_with_name(&edit.column).map_err(|_| {
                        recipe_error(format!("no column named {:?} to edit", edit.column))
                    })?;
                    let value = parse_value(edit.value.as_deref(), field.data_type())?;
                    cells.set(&edit.column, edit.row, value);
                }
                Step::EditCells(cells)
            }
            RecipeStep::InsertRows { at, count } => Step::InsertRows { at, count },
            RecipeStep::DeleteRows { rows } => Step::DeleteRows {
                rows: rows.into_iter().map(|[a, b]| a..b).collect(),
            },
            RecipeStep::AddColumn {
                name,
                data_type,
                at,
            } => Step::AddColumn {
                name,
                data_type: parse_type(&data_type)?,
                at,
            },
            RecipeStep::RemoveColumns { names } => Step::RemoveColumns { names },
            RecipeStep::KeepColumns { names } => Step::KeepColumns { names },
            RecipeStep::RenameColumn { from, to } => Step::RenameColumn { from, to },
            RecipeStep::MoveColumn { name, to } => Step::MoveColumn { name, to },
            RecipeStep::Filter {
                match_all,
                exclude,
                conditions,
            } => Step::Filter(Filter {
                match_all,
                exclude,
                conditions: conditions
                    .into_iter()
                    .map(|c| Condition {
                        column: c.column,
                        op: c.op.into(),
                        value: c.value,
                        case_sensitive: c.case_sensitive,
                    })
                    .collect(),
            }),
            RecipeStep::Sort { keys } => Step::Sort(Sort {
                keys: keys
                    .into_iter()
                    .map(|k| SortKey {
                        column: k.column,
                        descending: k.descending,
                        nulls_first: k.nulls_first,
                    })
                    .collect(),
            }),
            RecipeStep::Cast {
                column,
                data_type,
                decimal,
                thousands,
                format,
                on_error,
            } => Step::Cast(Cast {
                column,
                to: parse_type(&data_type)?,
                options: CastOptions {
                    decimal,
                    thousands,
                    format,
                    on_error: on_error.into(),
                },
            }),
            RecipeStep::Replace {
                column,
                find,
                with,
                whole_cell,
            } => Step::Replace(Replace {
                column,
                find,
                with,
                whole_cell,
            }),
            RecipeStep::FormatText { columns, op } => Step::FormatText {
                columns,
                op: op.into(),
            },
            RecipeStep::Fill { columns, up } => Step::Fill(Fill { columns, up }),
            RecipeStep::Split {
                column,
                delimiter,
                parts,
                positions,
            } => {
                let by = match (delimiter, parts, positions) {
                    (Some(delimiter), Some(parts), None) => SplitBy::Delimiter { delimiter, parts },
                    (None, None, Some(positions)) => SplitBy::Positions(positions),
                    _ => {
                        return Err(recipe_error(
                            "split needs either delimiter and parts, or positions",
                        ));
                    }
                };
                Step::Split(Split { column, by })
            }
            RecipeStep::Merge {
                columns,
                separator,
                name,
            } => Step::Merge(Merge {
                columns,
                separator,
                name,
            }),
            RecipeStep::RemoveEmptyColumn { name } => Step::RemoveEmptyColumn { name },
            RecipeStep::ExpandStruct { column, all_levels } => {
                Step::ExpandStruct { column, all_levels }
            }
            RecipeStep::ExplodeList { column } => Step::ExplodeList { column },
            RecipeStep::CombineStruct { columns, name } => Step::CombineStruct { columns, name },
            RecipeStep::CollectList { keys, columns } => Step::CollectList { keys, columns },
            RecipeStep::CustomColumn {
                name,
                expression,
                at,
            } => Step::CustomColumn(CustomColumn {
                name,
                expression,
                at,
            }),
        })
    }
}

/// Writes steps as a JSON recipe.
pub fn to_json(steps: &[Step], info: &RecipeInfo) -> Result<String> {
    let recipe = Recipe {
        format: FORMAT.into(),
        version: VERSION,
        name: info.name.clone(),
        description: info.description.clone(),
        created_with: format!("veta {}", env!("CARGO_PKG_VERSION")),
        source_file: info.source_file.clone(),
        steps: steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                Ok(RecipeEntry {
                    order: i + 1,
                    description: step.describe(),
                    step: RecipeStep::from_step(step)?,
                })
            })
            .collect::<Result<_>>()?,
    };
    serde_json::to_string_pretty(&recipe).map_err(recipe_error)
}

/// Reads a JSON recipe as steps to apply on data with `schema`, in order.
pub fn from_json(text: &str, schema: &SchemaRef) -> Result<Vec<Step>> {
    let recipe: Recipe = serde_json::from_str(text).map_err(recipe_error)?;
    if recipe.format != FORMAT {
        return Err(recipe_error(format!(
            "this is not a Veta recipe (format {:?})",
            recipe.format
        )));
    }
    if recipe.version != VERSION {
        return Err(recipe_error(format!(
            "version {} is not supported (expected {VERSION})",
            recipe.version
        )));
    }
    let mut entries = recipe.steps;
    entries.sort_by_key(|e| e.order);
    let mut schema: SchemaRef = Arc::clone(schema);
    let mut steps = Vec::with_capacity(entries.len());
    for entry in entries {
        let order = entry.order;
        let step = entry
            .step
            .into_step(&schema)
            .map_err(|e| recipe_error(format!("step {order}: {e}")))?;
        // A step that doesn't apply keeps the schema; it shows as broken
        // once loaded.
        if let Ok(next) = schema_after(&schema, &step) {
            schema = next;
        }
        steps.push(step);
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, Int64Array};
    use arrow::datatypes::{Field, Schema, TimeUnit};

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]))
    }

    fn all_steps() -> Vec<Step> {
        let mut edits = CellEdits::default();
        edits.set("id", 3, Arc::new(Int64Array::from(vec![42])));
        edits.set("name", 0, arrow::array::new_null_array(&DataType::Utf8, 1));
        vec![
            Step::EditCells(edits),
            Step::InsertRows { at: 1, count: 2 },
            Step::DeleteRows {
                rows: vec![0..1, 4..6],
            },
            Step::AddColumn {
                name: "when".into(),
                data_type: DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                at: 2,
            },
            Step::RenameColumn {
                from: "name".into(),
                to: "label".into(),
            },
            Step::MoveColumn {
                name: "label".into(),
                to: 0,
            },
            Step::Filter(Filter {
                match_all: false,
                exclude: false,
                conditions: vec![Condition {
                    column: "id".into(),
                    op: FilterOp::GreaterOrEqual,
                    value: "2".into(),
                    case_sensitive: true,
                }],
            }),
            Step::Sort(Sort {
                keys: vec![SortKey {
                    column: "id".into(),
                    descending: true,
                    nulls_first: false,
                }],
            }),
            Step::Cast(Cast {
                column: "label".into(),
                to: DataType::Float64,
                options: CastOptions {
                    decimal: ',',
                    thousands: Some('.'),
                    format: None,
                    on_error: OnError::ErrorColumn,
                },
            }),
            Step::Replace(Replace {
                column: "label errors".into(),
                find: None,
                with: Some("ok".into()),
                whole_cell: true,
            }),
            Step::FormatText {
                columns: vec!["label errors".into()],
                op: TextOp::Title,
            },
            Step::Fill(Fill {
                columns: vec!["id".into()],
                up: true,
            }),
            Step::Split(Split {
                column: "label errors".into(),
                by: SplitBy::Positions(vec![1, 2]),
            }),
            Step::Merge(Merge {
                columns: vec!["label errors.1".into(), "label errors.2".into()],
                separator: "-".into(),
                name: "m".into(),
            }),
            Step::CombineStruct {
                columns: vec!["id".into(), "m".into()],
                name: "s".into(),
            },
            Step::CollectList {
                keys: vec!["s".into()],
                columns: vec!["label".into()],
            },
            Step::CustomColumn(CustomColumn {
                name: "c".into(),
                expression: "id * 2".into(),
                at: 0,
            }),
            Step::RemoveColumns {
                names: vec!["c".into()],
            },
        ]
    }

    #[test]
    fn round_trips_every_step() {
        let steps = all_steps();
        let info = RecipeInfo {
            name: "test".into(),
            source_file: Some("a.parquet".into()),
            ..RecipeInfo::default()
        };
        let text = to_json(&steps, &info).unwrap();
        assert!(text.contains("\"type\": \"edit_cells\""), "{text}");
        assert!(
            text.contains("\"description\": \"Renamed name to label\""),
            "{text}"
        );
        let back = from_json(&text, &schema()).unwrap();
        assert_eq!(back.len(), steps.len());
        for (a, b) in steps.iter().zip(&back) {
            match (a, b) {
                // Values are rebuilt from text; compare them as text.
                (Step::EditCells(a), Step::EditCells(b)) => {
                    let text = |e: &CellEdits| {
                        e.iter()
                            .map(|(c, r, v)| (c.to_owned(), r, v.is_null(0), v.len()))
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(text(a), text(b));
                    assert_eq!(b.get("id", 3).unwrap().data_type(), &DataType::Int64);
                }
                _ => assert_eq!(a, b),
            }
        }
    }

    #[test]
    fn steps_run_by_order() {
        let text = r#"{
          "format": "veta-recipe",
          "version": 1,
          "steps": [
            { "order": 2, "type": "rename_column", "settings": { "from": "x", "to": "y" } },
            { "order": 1, "type": "rename_column", "settings": { "from": "name", "to": "x" } }
          ]
        }"#;
        let steps = from_json(text, &schema()).unwrap();
        assert_eq!(
            steps[0],
            Step::RenameColumn {
                from: "name".into(),
                to: "x".into()
            }
        );
    }

    #[test]
    fn reports_bad_recipes() {
        let s = schema();
        let recipe = |version: u32, steps: &str| {
            format!(r#"{{"format": "veta-recipe", "version": {version}, "steps": [{steps}]}}"#)
        };
        assert!(
            from_json(&recipe(2, ""), &s)
                .unwrap_err()
                .to_string()
                .contains("version")
        );
        assert!(from_json(r#"{"format": "x", "version": 1}"#, &s).is_err());
        assert!(from_json(&recipe(1, r#"{"order": 1, "type": "explode"}"#), &s).is_err());
        let bad_value = recipe(
            1,
            r#"{"order": 1, "type": "edit_cells", "settings": {"edits": [{"row": 0, "column": "id", "value": "x"}]}}"#,
        );
        let err = from_json(&bad_value, &s).unwrap_err().to_string();
        assert!(err.contains("step 1"), "{err}");
        assert!(from_json(&recipe(1, ""), &s).unwrap().is_empty());
    }
}
