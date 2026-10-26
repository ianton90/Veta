//! Tests for `files::recipe`.

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
            case_sensitive: true,
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
        Step::AddField {
            column: "s".into(),
            path: Vec::new(),
            name: "extra".into(),
            data_type: DataType::new_list(DataType::Int64, true),
        },
        Step::AddField {
            column: "s".into(),
            path: vec![
                crate::display::PathPart::Field("extra".into()),
                crate::display::PathPart::Index(0),
            ],
            name: "deep".into(),
            data_type: DataType::Int64,
        },
        Step::RemoveField {
            column: "s".into(),
            path: Vec::new(),
            name: "extra".into(),
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
    };
    let text = to_json(&steps, &info).unwrap();
    assert!(text.contains("\"type\": \"edit_cells\""), "{text}");
    assert!(!text.contains("description"), "{text}");
    // Older recipes described the recipe and each step: ignored.
    let old = text
        .replacen("\"name\"", "\"description\": \"x\",\n  \"name\"", 1)
        .replace("\"order\"", "\"description\": \"y\",\n      \"order\"");
    assert_eq!(from_json(&old, &schema()).unwrap().len(), steps.len());
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
