//! Controller tests: nested values and struct/list columns.

use super::*;
use crate::display::PathPart;

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
            Command::AddStep(Step::ExpandStruct {
                column: "point".into(),
                all_levels: false,
            }),
        )
        .unwrap();
        assert_eq!(cell(&doc, 2, "point.x").as_deref(), Some("10"));
        execute(
            &mut doc,
            Command::AddStep(Step::ExplodeList {
                column: "tags".into(),
            }),
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
                Command::AddStep(Step::ExpandStruct {
                    column: "tags".into(),
                    all_levels: false,
                })
            )
            .is_err()
        );
        // All levels: owner → owner.name, owner.home → owner.home.x, ….
        execute(
            &mut doc,
            Command::AddStep(Step::ExpandStruct {
                column: "owner".into(),
                all_levels: true,
            }),
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
        let collect = |keys: &[&str], columns: &[&str]| {
            Command::AddStep(Step::CollectList {
                keys: keys.iter().map(|c| c.to_string()).collect(),
                columns: columns.iter().map(|c| c.to_string()).collect(),
            })
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
        assert!(matches!(
            &doc.steps()[0],
            crate::steps::Step::CollectList { keys, columns }
                if keys == &["name"] && columns.len() == 2
        ));

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
        Command::AddStep(Step::CollectList {
            keys: Vec::new(),
            columns: vec!["id".into()],
        }),
    )
    .unwrap();
    doc.compute_all().unwrap();
    assert_eq!(doc.num_rows(), 1);
    let batch = doc.read(0..1).unwrap();
    assert_eq!(
        crate::display::item_count(batch.column(0).as_ref(), 0),
        rows
    );

    // Groups that span several reads keep all their rows, in order.
    let path = dir.join("groups.parquet");
    let rows = 3 * 64 * 1024 + 10;
    fixtures::large(&path, rows, 50_000);
    let mut doc = Document::open(&path, OpenOptions::paged()).unwrap();
    let key = crate::formulas::CustomColumn {
        name: "k".into(),
        expression: "[id] % 3".into(),
        at: 0,
    };
    execute(&mut doc, Command::AddStep(Step::CustomColumn(key))).unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::CollectList {
            keys: vec!["k".into()],
            columns: vec!["id".into()],
        }),
    )
    .unwrap();
    doc.compute_all().unwrap();
    assert_eq!(doc.num_rows(), 3);
    let batch = doc.read(0..3).unwrap();
    let ids = batch.column_by_name("id").unwrap();
    let counts: Vec<usize> = (0..3)
        .map(|r| crate::display::item_count(ids.as_ref(), r))
        .collect();
    assert_eq!(counts, [65540, 65539, 65539]);
    let first = ids
        .as_any()
        .downcast_ref::<arrow::array::ListArray>()
        .unwrap()
        .value(1);
    let first = first
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap();
    assert!(
        first
            .values()
            .iter()
            .copied()
            .eq((1..rows as i64).step_by(3))
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
        (
            "scores",
            crate::display::map_type(DataType::Utf8, DataType::Int64),
        ),
        (
            "names",
            crate::display::map_type(DataType::Int64, DataType::Utf8),
        ),
    ] {
        execute(
            &mut doc,
            Command::AddStep(Step::AddColumn {
                name: name.into(),
                data_type,
                at: 0,
            }),
        )
        .unwrap();
    }
    execute(&mut doc, set_cell(1, "point", Some(r#"{"x": 1.5}"#))).unwrap();
    execute(&mut doc, set_cell(2, "tags", Some("[1, 2]"))).unwrap();
    execute(&mut doc, insert(1, "scores", vec![], 0, Some("math"))).unwrap();
    execute(
        &mut doc,
        set(1, "scores", vec![PathPart::Entry(0)], Some("9")),
    )
    .unwrap();
    // A map with whole-number keys.
    execute(&mut doc, insert(0, "names", vec![], 0, Some("7"))).unwrap();
    execute(
        &mut doc,
        set(0, "names", vec![PathPart::Entry(0)], Some("x")),
    )
    .unwrap();
    execute(&mut doc, rename_key(0, "names", vec![], 0, "8")).unwrap();
    assert!(execute(&mut doc, insert(0, "names", vec![], 1, Some("eight"))).is_err());
    execute(
        &mut doc,
        set_cell(2, "names", Some(r#"{"1": "a", "2": null}"#)),
    )
    .unwrap();
    let out = dir.join("nested-new.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(
        cell(&saved, 1, "point").as_deref(),
        Some("{x: 1.5, y: null}")
    );
    assert_eq!(cell(&saved, 2, "tags").as_deref(), Some("[1, 2]"));
    assert_eq!(cell(&saved, 1, "scores").as_deref(), Some("{math: 9}"));
    assert_eq!(cell(&saved, 0, "point"), None);
    assert_eq!(cell(&saved, 0, "scores"), None);
    assert_eq!(cell(&saved, 0, "names").as_deref(), Some("{8: x}"));
    assert_eq!(cell(&saved, 2, "names").as_deref(), Some("{1: a, 2: null}"));
}

#[test]
fn combine_columns_into_struct() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        let combine = |columns: &[&str], name: &str| {
            Command::AddStep(Step::CombineStruct {
                columns: columns.iter().map(|c| c.to_string()).collect(),
                name: name.into(),
            })
        };
        // Reverses Expand: `point.` is dropped from the field names.
        execute(
            &mut doc,
            Command::AddStep(Step::ExpandStruct {
                column: "point".into(),
                all_levels: false,
            }),
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
        assert!(matches!(
            &doc.steps()[2],
            crate::steps::Step::CombineStruct { columns, name }
                if columns.len() == 2 && name == "rec"
        ));

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

fn f(name: &str) -> PathPart {
    PathPart::Field(name.into())
}

fn set(row: usize, column: &str, path: Vec<PathPart>, value: Option<&str>) -> Command {
    Command::SetValue {
        row,
        column: column.into(),
        path,
        value: value.map(Into::into),
    }
}

fn insert(
    row: usize,
    column: &str,
    path: Vec<PathPart>,
    index: usize,
    key: Option<&str>,
) -> Command {
    Command::InsertItem {
        row,
        column: column.into(),
        path,
        index,
        key: key.map(Into::into),
    }
}

fn remove(row: usize, column: &str, path: Vec<PathPart>, index: usize) -> Command {
    Command::RemoveItem {
        row,
        column: column.into(),
        path,
        index,
    }
}

fn rename_key(row: usize, column: &str, path: Vec<PathPart>, index: usize, key: &str) -> Command {
    Command::SetKey {
        row,
        column: column.into(),
        path,
        index,
        key: key.into(),
    }
}

#[test]
fn struct_fields_and_list_items_by_path() {
    use PathPart::Index;
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        execute(&mut doc, set(0, "point", vec![f("x")], Some("5"))).unwrap();
        execute(&mut doc, set(0, "point", vec![f("y")], None)).unwrap();
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 5, y: null}"));
        assert_eq!(doc.steps().len(), 1, "edits join one step");
        execute(
            &mut doc,
            set(0, "owner", vec![f("home"), f("x")], Some("9")),
        )
        .unwrap();
        assert!(cell(&doc, 0, "owner").unwrap().contains("x: 9"));
        // A field under a null struct creates it.
        execute(
            &mut doc,
            set(2, "owner", vec![f("home"), f("y")], Some("1")),
        )
        .unwrap();
        assert_eq!(
            cell(&doc, 2, "owner").as_deref(),
            Some("{name: null, home: {x: null, y: 1}}")
        );
        for bad in [
            set(0, "point", vec![f("x")], Some("abc")),
            set(0, "point", vec![f("z")], Some("1")),
            set(9, "point", vec![f("x")], Some("1")),
            set(0, "nope", vec![f("x")], Some("1")),
            set(0, "point", vec![], Some("1")),
        ] {
            assert!(execute(&mut doc, bad).is_err());
        }

        // List items: set, append, insert, remove; a null list is empty.
        let before = cell(&doc, 0, "tags").unwrap();
        execute(&mut doc, set(0, "tags", vec![Index(1)], Some("z"))).unwrap();
        execute(&mut doc, set(0, "tags", vec![Index(2)], Some("w"))).unwrap();
        assert_eq!(cell(&doc, 0, "tags").as_deref(), Some("[a, z, w]"));
        execute(&mut doc, insert(0, "tags", vec![], 0, None)).unwrap();
        assert_eq!(cell(&doc, 0, "tags").as_deref(), Some("[null, a, z, w]"));
        execute(&mut doc, remove(0, "tags", vec![], 3)).unwrap();
        execute(&mut doc, remove(0, "tags", vec![], 0)).unwrap();
        assert_eq!(cell(&doc, 0, "tags").as_deref(), Some("[a, z]"));
        assert!(execute(&mut doc, remove(0, "tags", vec![], 5)).is_err());
        assert!(execute(&mut doc, set(0, "tags", vec![Index(5)], Some("q"))).is_err());
        execute(&mut doc, insert(2, "tags", vec![], 0, None)).unwrap();
        assert_eq!(cell(&doc, 2, "tags").as_deref(), Some("[null]"));
        assert!(execute(&mut doc, insert(0, "point", vec![], 0, None)).is_err());
        // A field of a struct inside a list.
        execute(&mut doc, set(0, "path", vec![Index(1), f("y")], Some("40"))).unwrap();
        assert_eq!(
            cell(&doc, 0, "path").as_deref(),
            Some("[{x: 0, y: 0}, {x: 3, y: 40}]")
        );

        while undo(&mut doc) {}
        assert_eq!(cell(&doc, 0, "tags").unwrap(), before);
        assert_eq!(cell(&doc, 0, "point").as_deref(), Some("{x: 1, y: 2}"));
    }
}

#[test]
fn map_entries_by_path() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
    let scores = |doc: &Document, row| cell(doc, row, "scores");
    let before = scores(&doc, 0).unwrap();
    let entry = PathPart::Entry;
    execute(&mut doc, set(0, "scores", vec![entry(1)], Some("8"))).unwrap();
    execute(&mut doc, rename_key(0, "scores", vec![], 0, "maths")).unwrap();
    assert_eq!(scores(&doc, 0).as_deref(), Some("{maths: 9, art: 8}"));
    execute(&mut doc, insert(0, "scores", vec![], 2, Some("pe"))).unwrap();
    execute(&mut doc, set(0, "scores", vec![entry(2)], Some("5"))).unwrap();
    assert_eq!(
        scores(&doc, 0).as_deref(),
        Some("{maths: 9, art: 8, pe: 5}")
    );
    execute(&mut doc, remove(0, "scores", vec![], 0)).unwrap();
    assert_eq!(scores(&doc, 0).as_deref(), Some("{art: 8, pe: 5}"));
    execute(&mut doc, insert(2, "scores", vec![], 0, Some("math"))).unwrap();
    assert_eq!(scores(&doc, 2).as_deref(), Some("{math: null}"));
    for bad in [
        rename_key(0, "scores", vec![], 0, "pe"),
        insert(0, "scores", vec![], 0, Some("art")),
        insert(0, "scores", vec![], 0, None),
        set(0, "scores", vec![entry(0)], Some("x")),
        set(0, "scores", vec![entry(9)], Some("1")),
        remove(1, "scores", vec![], 0),
        rename_key(0, "tags", vec![], 0, "k"),
    ] {
        assert!(execute(&mut doc, bad).is_err());
    }
    while undo(&mut doc) {}
    assert_eq!(scores(&doc, 0).unwrap(), before);
}

#[test]
fn values_nested_to_any_depth() {
    use PathPart::{Entry, Index};
    let dir = TempDir::new();
    let path = dir.join("deep.parquet");
    fixtures::deep_nested(&path, 50);
    let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
    // A row whose first order has options, and one with a matrix.
    let row = (0..50)
        .find(|&r| {
            cell(&doc, r, "orders").is_some_and(|o| o.starts_with("[{") && o.contains("color"))
        })
        .unwrap();
    // orders[0].options: rename a key, set its value, add an entry.
    let options = vec![Index(0), f("options")];
    execute(
        &mut doc,
        rename_key(row, "orders", options.clone(), 0, "colour"),
    )
    .unwrap();
    let mut value = options.clone();
    value.push(Entry(0));
    execute(&mut doc, set(row, "orders", value, Some("teal"))).unwrap();
    execute(
        &mut doc,
        insert(row, "orders", options.clone(), 2, Some("fit")),
    )
    .unwrap();
    let orders = cell(&doc, row, "orders").unwrap();
    assert!(
        orders.contains("options: {colour: teal, size: "),
        "{orders}"
    );
    assert!(orders.contains("fit: null}"), "{orders}");
    // A list inside a list: append to the first inner list, add a new one.
    execute(&mut doc, set(3, "matrix", vec![], None)).unwrap_err();
    execute(&mut doc, insert(1, "matrix", vec![], 0, None)).unwrap();
    execute(
        &mut doc,
        set(1, "matrix", vec![Index(0), Index(0)], Some("42")),
    )
    .unwrap();
    assert!(cell(&doc, 1, "matrix").unwrap().starts_with("[[42], "));
    // An int-keyed map of structs holding lists.
    let history = (0..50)
        .find(|&r| cell(&doc, r, "history").is_some_and(|h| h.contains("notes: [")))
        .unwrap();
    execute(&mut doc, insert(history, "history", vec![], 0, Some("1"))).unwrap();
    execute(
        &mut doc,
        set(history, "history", vec![Entry(0), f("status")], Some("new")),
    )
    .unwrap();
    execute(
        &mut doc,
        insert(history, "history", vec![Entry(0), f("notes")], 0, None),
    )
    .unwrap();
    execute(
        &mut doc,
        set(
            history,
            "history",
            vec![Entry(0), f("notes"), Index(0)],
            Some("hi"),
        ),
    )
    .unwrap();
    assert!(
        cell(&doc, history, "history")
            .unwrap()
            .starts_with("{1: {status: new, notes: [hi]}"),
        "{:?}",
        cell(&doc, history, "history")
    );
    // The struct column: a list of structs inside it.
    execute(&mut doc, insert(4, "profile", vec![f("children")], 0, None)).unwrap();
    execute(
        &mut doc,
        set(
            4,
            "profile",
            vec![f("children"), Index(0), f("name")],
            Some("Zoe"),
        ),
    )
    .unwrap();
    assert!(
        cell(&doc, 4, "profile")
            .unwrap()
            .contains("children: [{name: Zoe, age: null}")
    );
    let out = dir.join("deep-edited.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(cell(&saved, 1, "matrix"), cell(&doc, 1, "matrix"));
    assert_eq!(
        cell(&saved, history, "history"),
        cell(&doc, history, "history")
    );
}

#[test]
fn fixed_size_lists() {
    use PathPart::Index;
    let dir = TempDir::new();
    let mut doc = mixed(&dir);
    let xyz = crate::display::parse_type_name("fixed_size_list<float64, 3>").unwrap();
    execute(
        &mut doc,
        Command::AddStep(Step::AddColumn {
            name: "xyz".into(),
            data_type: xyz,
            at: 0,
        }),
    )
    .unwrap();
    // Setting an item of a null fixed-size list fills the rest with nulls.
    execute(&mut doc, set(0, "xyz", vec![Index(1)], Some("2.5"))).unwrap();
    assert_eq!(cell(&doc, 0, "xyz").as_deref(), Some("[null, 2.5, null]"));
    execute(&mut doc, set(0, "xyz", vec![Index(0)], Some("1"))).unwrap();
    assert_eq!(cell(&doc, 0, "xyz").as_deref(), Some("[1.0, 2.5, null]"));
    for bad in [
        set(0, "xyz", vec![Index(3)], Some("1")),
        insert(0, "xyz", vec![], 0, None),
        remove(0, "xyz", vec![], 0),
    ] {
        assert!(execute(&mut doc, bad).is_err());
    }
    let out = dir.join("xyz.parquet");
    doc.save(Some(out.clone())).unwrap();
    let saved = Document::open(&out, OpenOptions::default()).unwrap();
    assert_eq!(cell(&saved, 0, "xyz").as_deref(), Some("[1.0, 2.5, null]"));
    assert!(matches!(
        saved.schema().field_with_name("xyz").unwrap().data_type(),
        DataType::FixedSizeList(_, 3)
    ));
}

#[test]
fn struct_fields_at_any_depth() {
    use PathPart::{Entry, Index};
    let dir = TempDir::new();
    let path = dir.join("deep.parquet");
    fixtures::deep_nested(&path, 30);
    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut doc = Document::open(&path, options).unwrap();
        let type_of = |doc: &Document, column: &str| {
            crate::display::type_name(doc.schema().field_with_name(column).unwrap().data_type())
        };
        let add = |column: &str, path: Vec<PathPart>, name: &str| {
            Command::AddStep(Step::AddField {
                column: column.into(),
                path,
                name: name.into(),
                data_type: DataType::Int64,
            })
        };
        // Every order (a struct in a list) gets the field.
        execute(&mut doc, add("orders", vec![Index(0)], "discount")).unwrap();
        assert!(type_of(&doc, "orders").contains("options: map<string, string>, discount: int64>"));
        let row = (0..30)
            .find(|&r| cell(&doc, r, "orders").is_some_and(|o| o.starts_with("[{")))
            .unwrap();
        assert!(
            cell(&doc, row, "orders")
                .unwrap()
                .contains("discount: null}")
        );
        execute(
            &mut doc,
            set(row, "orders", vec![Index(0), f("discount")], Some("5")),
        )
        .unwrap();
        assert!(cell(&doc, row, "orders").unwrap().contains("discount: 5}"));
        // A struct inside a map's values, and inside a struct.
        execute(&mut doc, add("history", vec![Entry(0)], "by")).unwrap();
        assert!(type_of(&doc, "history").contains("notes: list<string>, by: int64>"));
        execute(
            &mut doc,
            Command::AddStep(Step::RemoveField {
                column: "history".into(),
                path: vec![Entry(0)],
                name: "notes".into(),
            }),
        )
        .unwrap();
        assert_eq!(
            type_of(&doc, "history"),
            "map<int64, struct<status: string, by: int64>>"
        );
        execute(&mut doc, add("address", vec![f("geo")], "alt")).unwrap();
        assert!(
            type_of(&doc, "address")
                .contains("geo: struct<lat: float64, lon: float64, alt: int64>")
        );
        execute(
            &mut doc,
            add("profile", vec![f("children"), Index(0)], "school"),
        )
        .unwrap();
        for bad in [
            add("orders", vec![], "x"),
            add("orders", vec![Index(0)], "sku"),
            add("tags", vec![Index(0)], "x"),
            add("address", vec![f("nope")], "x"),
        ] {
            assert!(execute(&mut doc, bad).is_err());
        }
        assert!(matches!(
            &doc.steps()[0],
            crate::steps::Step::AddField { name, .. } if name == "discount"
        ));
        let out = dir.join("deep-fields.parquet");
        doc.save(Some(out.clone())).unwrap();
        let saved = Document::open(&out, OpenOptions::default()).unwrap();
        assert_eq!(type_of(&saved, "history"), type_of(&doc, "history"));
        assert_eq!(cell(&saved, row, "orders"), cell(&doc, row, "orders"));
    }
}
