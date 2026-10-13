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
    *value_at(&mut root, pointer)? = leaf;
    Ok(root.to_string())
}

/// `json` indented for reading; returned as is if it isn't valid JSON.
pub fn pretty_json(json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| json.to_owned())
}

/// `text` as compact JSON, `None` for `null`. Fails if it isn't JSON.
pub fn compact_json(text: &str) -> Result<Option<String>> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Null) => Ok(None),
        Ok(value) => Ok(Some(value.to_string())),
        Err(e) => Err(Error::InvalidCommand(format!("not valid JSON: {e}"))),
    }
}

/// Ways to change the shape of a nested cell (see [`edit_nested`]).
#[derive(Debug, Clone, PartialEq)]
pub enum NestedEdit {
    /// Adds a null item at the end of the list at the pointer.
    AppendItem,
    /// Adds an entry with a new key and a null value to the map at the
    /// pointer.
    AddEntry,
    /// Removes the list item or map entry at the pointer.
    Remove,
    /// Renames the map entry at the pointer.
    RenameKey(String),
}

/// The JSON of a nested cell after `edit` at `pointer`. Struct fields can't
/// be added or removed (the column's type fixes them).
pub fn edit_nested(
    cell_json: &str,
    pointer: &[crate::display::PathPart],
    edit: &NestedEdit,
) -> Result<String> {
    use crate::display::PathPart;
    let invalid = |m: &str| Error::InvalidCommand(m.into());
    let mut root: serde_json::Value = serde_json::from_str(cell_json)
        .map_err(|e| Error::InvalidCommand(format!("the cell is not valid JSON: {e}")))?;
    match edit {
        NestedEdit::AppendItem => match value_at(&mut root, pointer)? {
            serde_json::Value::Array(items) => items.push(serde_json::Value::Null),
            serde_json::Value::Null => *value_at(&mut root, pointer)? = serde_json::json!([null]),
            _ => return Err(invalid("items can only be added to lists")),
        },
        NestedEdit::AddEntry => {
            let target = value_at(&mut root, pointer)?;
            if target.is_null() {
                *target = serde_json::json!({});
            }
            let serde_json::Value::Object(map) = target else {
                return Err(invalid("entries can only be added to maps"));
            };
            let key = (1..)
                .map(|n| {
                    if n == 1 {
                        "key".to_owned()
                    } else {
                        format!("key {n}")
                    }
                })
                .find(|k| !map.contains_key(k))
                .unwrap_or_default();
            map.insert(key, serde_json::Value::Null);
        }
        NestedEdit::Remove | NestedEdit::RenameKey(_) => {
            let Some((last, parent)) = pointer.split_last() else {
                return Err(invalid("pick an item or entry"));
            };
            let container = value_at(&mut root, parent)?;
            match (edit, last, container) {
                (NestedEdit::Remove, PathPart::Index(i), serde_json::Value::Array(items))
                    if *i < items.len() =>
                {
                    items.remove(*i);
                }
                (NestedEdit::Remove, PathPart::Key(key), serde_json::Value::Object(map)) => {
                    map.shift_remove(key)
                        .ok_or_else(|| invalid("that entry no longer exists"))?;
                }
                (
                    NestedEdit::RenameKey(new),
                    PathPart::Key(old),
                    serde_json::Value::Object(map),
                ) => {
                    let new = new.trim();
                    if new.is_empty() {
                        return Err(invalid("keys can't be empty"));
                    }
                    if new != old && map.contains_key(new) {
                        return Err(Error::InvalidCommand(format!(
                            "the key {new:?} already exists"
                        )));
                    }
                    let index = map
                        .keys()
                        .position(|k| k == old)
                        .ok_or_else(|| invalid("that entry no longer exists"))?;
                    let value = map.shift_remove(old).unwrap_or_default();
                    map.shift_insert(index, new.to_owned(), value);
                }
                (_, PathPart::Field(_), _) => {
                    return Err(invalid("struct fields can't be removed; set them to null"));
                }
                _ => return Err(invalid("that part of the value no longer exists")),
            }
        }
    }
    Ok(root.to_string())
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
        assert!(set_nested(owner, &home_y, Some("a"), &DataType::Int64).is_err());
        let tags = schema.field(2).data_type();
        let list = parse_value(Some(r#"["a", null]"#), tags).unwrap();
        assert_eq!(
            value_json(list.as_ref(), 0).unwrap().unwrap(),
            r#"["a",null]"#
        );
    }
}
