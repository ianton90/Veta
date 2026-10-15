//! Controller tests: nested values and struct/list columns.

use super::*;

#[test]
fn nested_cells_edit_as_json_and_expand_explode() {
    use crate::steps::Status;
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        execute(
            &mut doc,
            Command::SetCell {
                row: 2,
                column: "point".into(),
                value: Some(r#"{"x": 10, "y": 20}"#.into()),
            },
        )
        .unwrap();
        assert_eq!(cell(&doc, 2, "point").as_deref(), Some("{x: 10, y: 20}"));
        let bad = execute(
            &mut doc,
            Command::SetCell {
                row: 2,
                column: "tags".into(),
                value: Some("[1, oops]".into()),
            },
        );
        assert!(bad.unwrap_err().to_string().contains("JSON"));

        execute(
            &mut doc,
            Command::ExpandStruct {
                column: "point".into(),
                all_levels: false,
            },
        )
        .unwrap();
        assert_eq!(cell(&doc, 2, "point.x").as_deref(), Some("10"));
        execute(
            &mut doc,
            Command::ExplodeList {
                column: "tags".into(),
            },
        )
        .unwrap();
        assert_eq!(doc.status(), &Status::Computing { step: 2 });
        doc.compute_all().unwrap();
        assert_eq!(doc.num_rows(), 7);
        assert_eq!(cell(&doc, 1, "tags").as_deref(), Some("b"));
        assert_eq!(cell(&doc, 1, "point.y").as_deref(), Some("2"));
        assert!(
            execute(
                &mut doc,
                Command::ExpandStruct {
                    column: "tags".into(),
                    all_levels: false,
                }
            )
            .is_err()
        );
        // All levels: owner → owner.name, owner.home → owner.home.x, ….
        execute(
            &mut doc,
            Command::ExpandStruct {
                column: "owner".into(),
                all_levels: true,
            },
        )
        .unwrap();
        let schema = doc.schema();
        assert!(schema.field_with_name("owner.home.x").is_ok());
        assert!(schema.field_with_name("owner.home").is_err());
        assert_eq!(cell(&doc, 0, "owner.home.x").as_deref(), Some("5"));
        let out = dir.join("out.parquet");
        doc.save(Some(out.clone())).unwrap();
        assert_eq!(
            Document::open(&out, OpenOptions::default())
                .unwrap()
                .num_rows(),
            7
        );
    }
}

#[test]
fn collect_values_into_lists() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        let collect = |keys: &[&str], columns: &[&str]| Command::CollectList {
            keys: keys.iter().map(|c| c.to_string()).collect(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
        };
        execute(&mut doc, collect(&["name"], &["score", "id"])).unwrap();
        doc.compute_all().unwrap();
        let names: Vec<_> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(names, ["id", "name", "score"], "input order, flag dropped");
        assert_eq!(doc.num_rows(), 6, "five names and null");
        assert_eq!(cell(&doc, 0, "name").as_deref(), Some("ana"));
        assert!(
            cell(&doc, 0, "id")
                .unwrap()
                .starts_with("[5, 10, 15, 20, 25, 30, 40")
        );
        assert_eq!(cell(&doc, 5, "name"), None, "nulls group last");
        let batch = doc.read(0..6).unwrap();
        let items: usize = (0..6)
            .map(|r| crate::display::item_count(batch.column(0).as_ref(), r))
            .sum();
        assert_eq!(items, fixtures::MIXED_SETTINGS_ROWS);
        assert_eq!(
            doc.steps()[0].describe(),
            "Collected 2 columns into lists by name"
        );

        assert!(execute(&mut doc, collect(&["name"], &[])).is_err());
        assert!(execute(&mut doc, collect(&["name"], &["name"])).is_err());
    }

    // Without keys every row is one group, across reads.
    let path = dir.join("large.parquet");
    let rows = 64 * 1024 + 10; // more than one read of a full pass
    fixtures::large(&path, rows, 50_000);
    let mut doc = Document::open(&path, OpenOptions::paged()).unwrap();
    execute(
        &mut doc,
        Command::CollectList {
            keys: Vec::new(),
            columns: vec!["id".into()],
        },
    )
    .unwrap();
    doc.compute_all().unwrap();
    assert_eq!(doc.num_rows(), 1);
    let batch = doc.read(0..1).unwrap();
    assert_eq!(
        crate::display::item_count(batch.column(0).as_ref(), 0),
        rows
    );
}

#[test]
fn new_nested_columns_save() {
    use arrow::datatypes::{DataType, Field};
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let point = DataType::Struct(
        vec![
            Field::new("x", DataType::Float64, true),
            Field::new("y", DataType::Utf8, true),
        ]
        .into(),
    );
    for (name, data_type) in [
        ("point", point),
        ("tags", DataType::new_list(DataType::Int64, true)),
    ] {
        execute(
            &mut doc,
            Command::AddColumn {
                name: name.into(),
                data_type,
                at: 0,
            },
        )
        .unwrap();
    }
    execute(&mut doc, set_cell(1, "point", Some(r#"{"x": 1.5}"#))).unwrap();
    execute(&mut doc, set_cell(2, "tags", Some("[1, 2]"))).unwrap();
    let out = dir.join("nested-new.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(
        cell(&saved, 1, "point").as_deref(),
        Some("{x: 1.5, y: null}")
    );
    assert_eq!(cell(&saved, 2, "tags").as_deref(), Some("[1, 2]"));
    assert_eq!(cell(&saved, 0, "point"), None);
}

#[test]
fn combine_columns_into_struct() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        let combine = |columns: &[&str], name: &str| Command::CombineStruct {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            name: name.into(),
        };
        // Reverses Expand: `point.` is dropped from the field names.
        execute(
            &mut doc,
            Command::ExpandStruct {
                column: "point".into(),
                all_levels: false,
            },
        )
        .unwrap();
        execute(&mut doc, combine(&["point.x", "point.y"], "point")).unwrap();
        assert_eq!(doc.schema().index_of("point").unwrap(), 1);
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 1, y: 2}"));
        assert_eq!(
            cell(&doc, 2, "point").as_deref(),
            Some("{x: null, y: null}")
        );

        execute(&mut doc, combine(&["tags", "id"], "rec")).unwrap();
        let schema = doc.schema();
        assert_eq!(schema.field(0).name(), "rec", "where the first column was");
        assert_eq!(
            cell(&doc, 0, "rec").as_deref(),
            Some("{tags: [a, b], id: 0}")
        );
        assert_eq!(doc.steps()[2].describe(), "Combined 2 columns into rec");

        assert!(
            execute(&mut doc, combine(&["path"], "owner")).is_err(),
            "taken"
        );
        assert!(execute(&mut doc, combine(&["path", "path"], "p")).is_err());
        assert!(execute(&mut doc, combine(&[], "p")).is_err());
        assert!(execute(&mut doc, combine(&["nope"], "p")).is_err());

        let out = dir.join("combined.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(
            cell(&saved, 3, "rec").as_deref(),
            Some("{tags: [c, null, d], id: 3}")
        );
    }
}

#[test]
fn leaf_columns_of_nested_schema() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let doc = Document::open(&path, OpenOptions::default()).unwrap();
    let leaves = crate::io::leaf_columns(&doc.schema()).unwrap();
    let names: Vec<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    assert!(names.contains(&"point.x"), "{names:?}");
    assert!(
        names.iter().any(|n| n.starts_with("tags.list.")),
        "{names:?}"
    );
    assert!(names.contains(&"owner.home.x"), "{names:?}");
    let (_, x) = leaves.iter().find(|(p, _)| p == "point.x").unwrap();
    assert_eq!(x, &DataType::Int64);
}

#[test]
fn nested_field_and_list_item_commands() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        let names = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let set_field = |row, column: &str, path: &[&str], value: Option<&str>| Command::SetField {
            row,
            column: column.into(),
            path: names(path),
            value: value.map(Into::into),
        };
        execute(&mut doc, set_field(0, "point", &["x"], Some("5"))).unwrap();
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 5, y: 2}"));
        execute(&mut doc, set_field(0, "point", &["y"], None)).unwrap();
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 5, y: null}"));
        assert_eq!(doc.steps().len(), 1, "edits join one step");
        execute(&mut doc, set_field(0, "owner", &["home", "x"], Some("9"))).unwrap();
        assert!(cell(&doc, 0, "owner").unwrap().contains("x: 9"));
        for bad in [
            set_field(0, "point", &["x"], Some("abc")),
            set_field(0, "point", &["z"], Some("1")),
            set_field(9, "point", &["x"], Some("1")),
            set_field(0, "nope", &["x"], Some("1")),
        ] {
            assert!(execute(&mut doc, bad).is_err());
        }

        let list = |command: fn(usize, String, Vec<String>, usize) -> Command, row, index| {
            command(row, "tags".into(), Vec::new(), index)
        };
        let insert: fn(usize, String, Vec<String>, usize) -> Command =
            |row, column, list, index| Command::InsertListItem {
                row,
                column,
                list,
                index,
            };
        let remove: fn(usize, String, Vec<String>, usize) -> Command =
            |row, column, list, index| Command::RemoveListItem {
                row,
                column,
                list,
                index,
            };
        let set_item = |row, index, value: &str| Command::SetListItem {
            row,
            column: "tags".into(),
            list: Vec::new(),
            index,
            value: Some(value.into()),
        };
        let before = cell(&doc, 0, "tags").unwrap();
        execute(&mut doc, set_item(0, 1, "z")).unwrap();
        execute(&mut doc, set_item(0, 2, "w")).unwrap();
        assert_eq!(cell(&doc, 0, "tags").as_deref(), Some("[a, z, w]"));
        execute(&mut doc, list(insert, 0, 0)).unwrap();
        assert_eq!(cell(&doc, 0, "tags").as_deref(), Some("[null, a, z, w]"));
        execute(&mut doc, list(remove, 0, 3)).unwrap();
        execute(&mut doc, list(remove, 0, 0)).unwrap();
        assert_eq!(cell(&doc, 0, "tags").as_deref(), Some("[a, z]"));
        assert!(execute(&mut doc, list(remove, 0, 5)).is_err());
        // A null list counts as empty.
        execute(&mut doc, list(insert, 2, 0)).unwrap();
        assert_eq!(cell(&doc, 2, "tags").as_deref(), Some("[null]"));
        let not_a_list = Command::InsertListItem {
            row: 0,
            column: "point".into(),
            list: Vec::new(),
            index: 0,
        };
        assert!(execute(&mut doc, not_a_list).is_err());
        // A list of structs: set a field of an item through the cell's JSON.
        execute(
            &mut doc,
            Command::SetListItem {
                row: 0,
                column: "path".into(),
                list: Vec::new(),
                index: 0,
                value: Some(r#"{"x": 1, "y": 1}"#.into()),
            },
        )
        .unwrap();
        assert!(cell(&doc, 0, "path").unwrap().starts_with("[{x: 1, y: 1}"));

        while undo(&mut doc) {}
        assert_eq!(cell(&doc, 0, "tags").unwrap(), before);
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 1, y: 2}"));
    }
}
