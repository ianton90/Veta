//! Tests for `values::parse`.

use super::*;
use crate::display::value_json;
use arrow::array::Array;

#[test]
fn nested_values_round_trip_through_json() {
    let schema = veta_testkit::fixtures::nested_schema();
    for field in schema.fields().iter().skip(1) {
        let t = field.data_type();
        let example = json_example(t);
        let value = parse_value(Some(&example), t)
            .unwrap_or_else(|e| panic!("{}: {example}: {e}", field.name()));
        let back = value_json(value.as_ref(), 0).unwrap().unwrap();
        let again = parse_value(Some(&back), t).unwrap();
        assert_eq!(value.as_ref(), again.as_ref(), "{}", field.name());
    }
    let point = schema.field(1).data_type();
    let p = parse_value(Some(r#"{"x": 3, "y": null}"#), point).unwrap();
    assert_eq!(
        value_json(p.as_ref(), 0).unwrap().unwrap(),
        r#"{"x":3,"y":null}"#
    );
    assert!(parse_value(Some("{x: 3}"), point).is_err());
    assert!(parse_value(Some(r#"{"x": "a"}"#), point).is_err());
    assert!(parse_value(Some(r#"{"x": 1} junk"#), point).is_err());
    assert!(parse_value(Some(""), point).unwrap().is_null(0));
    // Editing one leaf inside a nested value.
    use crate::display::PathPart;
    let owner = r#"{"name":"Ana","home":{"x":5,"y":6}}"#;
    let home_y = [PathPart::Field("home".into()), PathPart::Field("y".into())];
    let edited = set_nested(owner, &home_y, Some("60"), &DataType::Int64).unwrap();
    let edited: serde_json::Value = serde_json::from_str(&edited).unwrap();
    assert_eq!(edited["home"]["y"], 60);
    assert_eq!(edited["name"], "Ana");
    let name = [PathPart::Field("name".into())];
    let cleared = set_nested(owner, &name, None, &DataType::Utf8).unwrap();
    assert!(cleared.contains(r#""name":null"#));
    // List items: set, append (also to a null list), insert, remove.
    let int = DataType::Int64;
    let set = |json: &str, index, text| {
        edit_list(
            json,
            &[],
            &ListEdit::Set {
                index,
                text,
                item_type: &int,
            },
        )
    };
    assert_eq!(set("[1,2]", 1, Some("5")).unwrap(), "[1,5]");
    assert_eq!(set("[1,2]", 2, Some("3")).unwrap(), "[1,2,3]");
    assert_eq!(set("null", 0, Some("7")).unwrap(), "[7]");
    assert_eq!(set("[1]", 0, None).unwrap(), "[null]");
    assert!(set("[1]", 3, Some("1")).is_err());
    assert!(set("[1]", 0, Some("x")).is_err());
    assert_eq!(
        edit_list("[1,2]", &[], &ListEdit::Insert(1)).unwrap(),
        "[1,null,2]"
    );
    assert_eq!(
        edit_list("[1,2]", &[], &ListEdit::Remove(0)).unwrap(),
        "[2]"
    );
    assert!(edit_list("[1]", &[], &ListEdit::Remove(1)).is_err());
    let tags = [PathPart::Field("tags".into())];
    assert_eq!(
        edit_list("null", &tags, &ListEdit::Insert(0)).unwrap(),
        r#"{"tags":[null]}"#
    );
    // Map entries: set a value, rename, insert, remove; keys stay unique.
    let text = DataType::Utf8;
    let scores = r#"{"math":9,"art":7}"#;
    let map = |edit: MapEdit| edit_map(scores, &[], &edit);
    let set = MapEdit::SetValue {
        index: 1,
        text: Some("8"),
        value_type: &int,
    };
    assert_eq!(map(set).unwrap(), r#"{"math":9,"art":8}"#);
    let rename = MapEdit::SetKey {
        index: 0,
        key: "maths",
        key_type: &text,
    };
    assert_eq!(map(rename).unwrap(), r#"{"maths":9,"art":7}"#);
    let same = MapEdit::SetKey {
        index: 0,
        key: "math",
        key_type: &text,
    };
    assert_eq!(map(same).unwrap(), scores);
    assert!(
        map(MapEdit::SetKey {
            index: 0,
            key: "art",
            key_type: &text,
        })
        .is_err()
    );
    let insert = MapEdit::Insert {
        index: 1,
        key: "pe",
        key_type: &text,
    };
    assert_eq!(map(insert).unwrap(), r#"{"math":9,"pe":null,"art":7}"#);
    assert!(
        map(MapEdit::Insert {
            index: 0,
            key: "art",
            key_type: &text,
        })
        .is_err()
    );
    let past = MapEdit::Insert {
        index: 3,
        key: "x",
        key_type: &text,
    };
    assert!(map(past).is_err());
    assert_eq!(map(MapEdit::Remove(0)).unwrap(), r#"{"art":7}"#);
    assert!(map(MapEdit::Remove(2)).is_err());
    // Other keys are read as their type and kept in JSON as text.
    let ids = r#"{"1":"a"}"#;
    let insert = |key| MapEdit::Insert {
        index: 1,
        key,
        key_type: &int,
    };
    assert_eq!(
        edit_map(ids, &[], &insert("02")).unwrap(),
        r#"{"1":"a","2":null}"#
    );
    assert!(edit_map(ids, &[], &insert("01")).is_err(), "01 is 1");
    assert!(edit_map(ids, &[], &insert("x")).is_err());
    assert!(edit_map(ids, &[], &insert("")).is_err());
    let by_id = crate::display::map_type(DataType::Int64, DataType::Utf8);
    let value = parse_value(Some(r#"{"7": "x", "3": null}"#), &by_id).unwrap();
    assert_eq!(
        value_json(value.as_ref(), 0).unwrap().unwrap(),
        r#"{"7":"x","3":null}"#
    );
    assert!(parse_value(Some(r#"{"x": "a"}"#), &by_id).is_err());
    let by_day = crate::display::map_type(DataType::Date32, DataType::Int64);
    let days = parse_value(Some(r#"{"2026-10-15": 1}"#), &by_day).unwrap();
    assert_eq!(
        value_json(days.as_ref(), 0).unwrap().unwrap(),
        r#"{"2026-10-15":1}"#
    );
    let first = MapEdit::Insert {
        index: 0,
        key: "a",
        key_type: &text,
    };
    assert_eq!(edit_map("null", &[], &first).unwrap(), r#"{"a":null}"#);
    // A field under a null struct is created.
    let created = set_nested("null", &home_y, Some("7"), &DataType::Int64).unwrap();
    assert_eq!(created, r#"{"home":{"y":7}}"#);
    assert!(set_nested(owner, &home_y, Some("a"), &DataType::Int64).is_err());
    let tags = schema.field(2).data_type();
    let list = parse_value(Some(r#"["a", null]"#), tags).unwrap();
    assert_eq!(
        value_json(list.as_ref(), 0).unwrap().unwrap(),
        r#"["a",null]"#
    );
}
