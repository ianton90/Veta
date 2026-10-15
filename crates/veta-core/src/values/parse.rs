//! Parsing user input into typed values.

use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray, new_null_array};
use arrow::compute::{CastOptions, cast_with_options};
use arrow::datatypes::DataType;

use crate::display::type_name;
use crate::error::{Error, Result};

pub fn is_text(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    )
}

/// Parses user input as a one-element array of `data_type`.
pub fn parse_value(text: Option<&str>, data_type: &DataType) -> Result<ArrayRef> {
    let text = match text {
        None => return Ok(new_null_array(data_type, 1)),
        Some(t) if t.trim().is_empty() && !is_text(data_type) => {
            return Ok(new_null_array(data_type, 1));
        }
        Some(t) if is_text(data_type) => t,
        Some(t) => t.trim(),
    };
    if data_type.is_nested() {
        return parse_json(text, data_type);
    }
    let input: ArrayRef = Arc::new(StringArray::from(vec![text]));
    let options = CastOptions {
        safe: false,
        ..CastOptions::default()
    };
    cast_with_options(&input, data_type, &options).map_err(|_| {
        Error::InvalidCommand(format!("{text:?} is not a valid {}", type_name(data_type)))
    })
}

/// Parses JSON text (`{"x": 1}`, `[1, 2]`, …) as a one-element array of a
/// nested type.
fn parse_json(text: &str, data_type: &DataType) -> Result<ArrayRef> {
    let invalid = |detail: String| {
        Error::InvalidCommand(format!(
            "not a valid {} (write it as JSON, e.g. {}): {detail}",
            type_name(data_type),
            json_example(data_type)
        ))
    };
    // Reject trailing text and non-JSON early, with a clear message.
    serde_json::from_str::<serde_json::Value>(text).map_err(|e| invalid(e.to_string()))?;
    let schema = Arc::new(arrow::datatypes::Schema::new(vec![
        arrow::datatypes::Field::new("v", data_type.clone(), true),
    ]));
    let line = format!("{{\"v\": {text}}}");
    let mut reader = arrow::json::ReaderBuilder::new(schema)
        .with_strict_mode(true)
        .build(std::io::Cursor::new(line))
        .map_err(|e| invalid(e.to_string()))?;
    let batch = reader
        .next()
        .ok_or_else(|| invalid("no value".into()))?
        .map_err(|e| invalid(e.to_string()))?;
    Ok(batch.column(0).clone())
}

/// The JSON of a nested cell with the leaf at `pointer` replaced by
/// `text` (read as `leaf_type`; empty or `None` sets null).
pub fn set_nested(
    cell_json: &str,
    pointer: &[crate::display::PathPart],
    text: Option<&str>,
    leaf_type: &DataType,
) -> Result<String> {
    let mut root: serde_json::Value = serde_json::from_str(cell_json)
        .map_err(|e| Error::InvalidCommand(format!("the cell is not valid JSON: {e}")))?;
    let leaf = parse_value(text, leaf_type)?;
    let leaf = match crate::display::value_json(leaf.as_ref(), 0)? {
        Some(json) => {
            serde_json::from_str(&json).map_err(|e| Error::InvalidCommand(e.to_string()))?
        }
        None => serde_json::Value::Null,
    };
    *value_at_or_insert(&mut root, pointer)? = leaf;
    Ok(root.to_string())
}

/// A change to one item of a list inside a cell (see [`edit_list`]).
#[derive(Debug, Clone, PartialEq)]
pub enum ListEdit<'a> {
    /// Sets the item at `index` (read as `item_type`; empty or `None` is
    /// null). `index` may be the list's length, which appends.
    Set {
        index: usize,
        text: Option<&'a str>,
        item_type: &'a DataType,
    },
    /// Inserts a null item at `index` (up to the list's length).
    Insert(usize),
    /// Removes the item at `index`.
    Remove(usize),
}

/// The JSON of a cell with the list at `list` changed by `edit`. A null
/// list (or struct on the way to it) counts as empty.
pub fn edit_list(
    cell_json: &str,
    list: &[crate::display::PathPart],
    edit: &ListEdit,
) -> Result<String> {
    let mut root: serde_json::Value = serde_json::from_str(cell_json)
        .map_err(|e| Error::InvalidCommand(format!("the cell is not valid JSON: {e}")))?;
    let target = value_at_or_insert(&mut root, list)?;
    if target.is_null() {
        *target = serde_json::Value::Array(Vec::new());
    }
    let serde_json::Value::Array(items) = target else {
        return Err(Error::InvalidCommand("that value is not a list".into()));
    };
    let gone = || Error::InvalidCommand("that item no longer exists".into());
    match *edit {
        ListEdit::Set {
            index,
            text,
            item_type,
        } => {
            let item = parse_value(text, item_type)?;
            let item = match crate::display::value_json(item.as_ref(), 0)? {
                Some(json) => {
                    serde_json::from_str(&json).map_err(|e| Error::InvalidCommand(e.to_string()))?
                }
                None => serde_json::Value::Null,
            };
            match index.cmp(&items.len()) {
                std::cmp::Ordering::Less => items[index] = item,
                std::cmp::Ordering::Equal => items.push(item),
                std::cmp::Ordering::Greater => return Err(gone()),
            }
        }
        ListEdit::Insert(index) if index <= items.len() => {
            items.insert(index, serde_json::Value::Null);
        }
        ListEdit::Remove(index) if index < items.len() => {
            items.remove(index);
        }
        ListEdit::Insert(_) | ListEdit::Remove(_) => return Err(gone()),
    }
    Ok(root.to_string())
}

/// A change to one entry of a map inside a cell (see [`edit_map`]). Entries
/// are addressed by position; keys are text and must stay unique.
#[derive(Debug, Clone, PartialEq)]
pub enum MapEdit<'a> {
    /// Sets the value of entry `index` (read as `value_type`; empty or
    /// `None` is null).
    SetValue {
        index: usize,
        text: Option<&'a str>,
        value_type: &'a DataType,
    },
    /// Renames the key of entry `index`.
    SetKey { index: usize, key: &'a str },
    /// Inserts an entry with a null value at `index` (up to the number of
    /// entries).
    Insert { index: usize, key: &'a str },
    /// Removes entry `index`.
    Remove(usize),
}

/// The JSON of a cell with the map at `map` changed by `edit`. A null map
/// (or struct on the way to it) counts as empty.
pub fn edit_map(
    cell_json: &str,
    map: &[crate::display::PathPart],
    edit: &MapEdit,
) -> Result<String> {
    let mut root: serde_json::Value = serde_json::from_str(cell_json)
        .map_err(|e| Error::InvalidCommand(format!("the cell is not valid JSON: {e}")))?;
    let target = value_at_or_insert(&mut root, map)?;
    if target.is_null() {
        *target = serde_json::Value::Object(Default::default());
    }
    let serde_json::Value::Object(object) = target else {
        return Err(Error::InvalidCommand("that value is not a map".into()));
    };
    let mut entries: Vec<(String, serde_json::Value)> =
        std::mem::take(object).into_iter().collect();
    let gone = || Error::InvalidCommand("that entry no longer exists".into());
    let taken =
        |entries: &[(String, serde_json::Value)], key: &str, except: Option<usize>| match entries
            .iter()
            .enumerate()
            .any(|(i, (k, _))| k == key && Some(i) != except)
        {
            true => Err(Error::InvalidCommand(format!("{key:?} is already a key"))),
            false => Ok(()),
        };
    match *edit {
        MapEdit::SetValue {
            index,
            text,
            value_type,
        } => {
            let value = parse_value(text, value_type)?;
            let value = match crate::display::value_json(value.as_ref(), 0)? {
                Some(json) => {
                    serde_json::from_str(&json).map_err(|e| Error::InvalidCommand(e.to_string()))?
                }
                None => serde_json::Value::Null,
            };
            entries.get_mut(index).ok_or_else(gone)?.1 = value;
        }
        MapEdit::SetKey { index, key } => {
            taken(&entries, key, Some(index))?;
            entries.get_mut(index).ok_or_else(gone)?.0 = key.to_owned();
        }
        MapEdit::Insert { index, key } if index <= entries.len() => {
            taken(&entries, key, None)?;
            entries.insert(index, (key.to_owned(), serde_json::Value::Null));
        }
        MapEdit::Remove(index) if index < entries.len() => {
            entries.remove(index);
        }
        MapEdit::Insert { .. } | MapEdit::Remove(_) => return Err(gone()),
    }
    *object = entries.into_iter().collect();
    Ok(root.to_string())
}

/// Like [`value_at`], but struct fields that are missing or under a null
/// parent are created.
fn value_at_or_insert<'a>(
    root: &'a mut serde_json::Value,
    pointer: &[crate::display::PathPart],
) -> Result<&'a mut serde_json::Value> {
    use crate::display::PathPart;
    let mut target = root;
    for part in pointer {
        target = match part {
            PathPart::Field(name) => {
                if target.is_null() {
                    *target = serde_json::Value::Object(Default::default());
                }
                match target {
                    serde_json::Value::Object(map) => {
                        map.entry(name.clone()).or_insert(serde_json::Value::Null)
                    }
                    _ => {
                        return Err(Error::InvalidCommand(
                            "that part of the value no longer exists".into(),
                        ));
                    }
                }
            }
            _ => value_at(target, std::slice::from_ref(part))?,
        };
    }
    Ok(target)
}

/// `json` indented for reading; returned as is if it isn't valid JSON.
pub fn pretty_json(json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| json.to_owned())
}

fn value_at<'a>(
    root: &'a mut serde_json::Value,
    pointer: &[crate::display::PathPart],
) -> Result<&'a mut serde_json::Value> {
    use crate::display::PathPart;
    let mut target = root;
    for part in pointer {
        let missing = || Error::InvalidCommand("that part of the value no longer exists".into());
        target = match part {
            PathPart::Field(name) | PathPart::Key(name) => {
                target.get_mut(name.as_str()).ok_or_else(missing)?
            }
            PathPart::Index(i) => target.get_mut(*i).ok_or_else(missing)?,
        };
    }
    Ok(target)
}

/// A short JSON example for a nested type.
pub fn json_example(data_type: &DataType) -> String {
    match data_type {
        DataType::Struct(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("\"{}\": {}", f.name(), json_example(f.data_type())))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
            format!("[{}]", json_example(item.data_type()))
        }
        DataType::Map(entries, _) => match entries.data_type() {
            DataType::Struct(kv) if kv.len() == 2 => {
                format!("{{\"key\": {}}}", json_example(kv[1].data_type()))
            }
            _ => "{}".into(),
        },
        t if is_text(t) => "\"text\"".into(),
        DataType::Boolean => "true".into(),
        t if t.is_floating() => "1.5".into(),
        t if t.is_integer() => "1".into(),
        DataType::Date32 | DataType::Date64 => "\"2024-01-31\"".into(),
        DataType::Timestamp(..) => "\"2024-01-31T12:00:00\"".into(),
        _ => "null".into(),
    }
}

#[cfg(test)]
mod tests {
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
        };
        assert_eq!(map(rename).unwrap(), r#"{"maths":9,"art":7}"#);
        let same = MapEdit::SetKey {
            index: 0,
            key: "math",
        };
        assert_eq!(map(same).unwrap(), scores);
        assert!(
            map(MapEdit::SetKey {
                index: 0,
                key: "art"
            })
            .is_err()
        );
        let insert = MapEdit::Insert {
            index: 1,
            key: "pe",
        };
        assert_eq!(map(insert).unwrap(), r#"{"math":9,"pe":null,"art":7}"#);
        assert!(
            map(MapEdit::Insert {
                index: 0,
                key: "art"
            })
            .is_err()
        );
        assert!(map(MapEdit::Insert { index: 3, key: "x" }).is_err());
        assert_eq!(map(MapEdit::Remove(0)).unwrap(), r#"{"art":7}"#);
        assert!(map(MapEdit::Remove(2)).is_err());
        let first = MapEdit::Insert { index: 0, key: "a" };
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
}
