//! Parsing user input into typed values.

use crate::error::invalid;
use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray, new_null_array};
use arrow::compute::{CastOptions, cast_with_options};
use arrow::datatypes::DataType;

use crate::display::type_name;
use crate::error::Result;

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
    cast_with_options(&input, data_type, &options)
        .map_err(|_| invalid!(InvalidValue, value = text, data_type = type_name(data_type)))
}

/// Parses JSON text (`{"x": 1}`, `[1, 2]`, …) as a one-element array of a
/// nested type.
fn parse_json(text: &str, data_type: &DataType) -> Result<ArrayRef> {
    // `detail`: what the JSON reader said.
    let invalid = |detail: String| {
        invalid!(
            NotValidJsonValue,
            data_type = type_name(data_type),
            example = json_example(data_type),
            detail = detail
        )
    };
    // Reject trailing text and non-JSON early, with a clear message.
    serde_json::from_str::<serde_json::Value>(text).map_err(|e| invalid(e.to_string()))?;
    // Maps with other than text keys are read with text keys, then cast.
    let json_type = crate::display::json_type(data_type);
    let read_type = json_type.as_ref().unwrap_or(data_type);
    let schema = Arc::new(arrow::datatypes::Schema::new(vec![
        arrow::datatypes::Field::new("v", read_type.clone(), true),
    ]));
    let line = format!("{{\"v\": {text}}}");
    let mut reader = arrow::json::ReaderBuilder::new(schema)
        .with_strict_mode(true)
        .build(std::io::Cursor::new(line))
        .map_err(|e| invalid(e.to_string()))?;
    let batch = reader
        .next()
        .ok_or_else(|| {
            invalid!(
                NotValidJson,
                data_type = type_name(data_type),
                example = json_example(data_type)
            )
        })?
        .map_err(|e| invalid(e.to_string()))?;
    let value = batch.column(0).clone();
    if json_type.is_none() {
        return Ok(value);
    }
    let options = CastOptions {
        safe: false,
        ..CastOptions::default()
    };
    cast_with_options(&value, data_type, &options)
        .map_err(|e| invalid!(KeyType, data_type = type_name(data_type), error = e))
}

/// The JSON of a nested cell with the leaf at `pointer` replaced by
/// `text` (read as `leaf_type`; empty or `None` sets null).
pub fn set_nested(
    cell_json: &str,
    pointer: &[crate::display::PathPart],
    text: Option<&str>,
    leaf_type: &DataType,
) -> Result<String> {
    let mut root: serde_json::Value =
        serde_json::from_str(cell_json).map_err(|e| invalid!(CellNotJson, error = e))?;
    let leaf = parse_value(text, leaf_type)?;
    let leaf = match crate::display::value_json(leaf.as_ref(), 0)? {
        Some(json) => serde_json::from_str(&json).map_err(|e| invalid!(Json, error = e))?,
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
    let mut root: serde_json::Value =
        serde_json::from_str(cell_json).map_err(|e| invalid!(CellNotJson, error = e))?;
    let target = value_at_or_insert(&mut root, list)?;
    if target.is_null() {
        *target = serde_json::Value::Array(Vec::new());
    }
    let serde_json::Value::Array(items) = target else {
        return Err(invalid!(ValueNotList));
    };
    let gone = || invalid!(ItemGone);
    match *edit {
        ListEdit::Set {
            index,
            text,
            item_type,
        } => {
            let item = parse_value(text, item_type)?;
            let item = match crate::display::value_json(item.as_ref(), 0)? {
                Some(json) => serde_json::from_str(&json).map_err(|e| invalid!(Json, error = e))?,
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

/// The JSON of a cell with the list at `list` set to `len` nulls if it is
/// null (or missing under a struct), so that a fixed-size list can have an
/// item set.
pub fn fill_null_list(
    cell_json: &str,
    list: &[crate::display::PathPart],
    len: usize,
) -> Result<String> {
    let mut root: serde_json::Value =
        serde_json::from_str(cell_json).map_err(|e| invalid!(CellNotJson, error = e))?;
    let target = value_at_or_insert(&mut root, list)?;
    if target.is_null() {
        *target = serde_json::Value::Array(vec![serde_json::Value::Null; len]);
    }
    Ok(root.to_string())
}

/// A change to one entry of a map inside a cell (see [`edit_map`]). Entries
/// are addressed by position; keys can't be null and must stay unique.
#[derive(Debug, Clone, PartialEq)]
pub enum MapEdit<'a> {
    /// Sets the value of entry `index` (read as `value_type`; empty or
    /// `None` is null).
    SetValue {
        index: usize,
        text: Option<&'a str>,
        value_type: &'a DataType,
    },
    /// Renames the key of entry `index` (read as `key_type`).
    SetKey {
        index: usize,
        key: &'a str,
        key_type: &'a DataType,
    },
    /// Inserts an entry with a null value at `index` (up to the number of
    /// entries).
    Insert {
        index: usize,
        key: &'a str,
        key_type: &'a DataType,
    },
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
    let mut root: serde_json::Value =
        serde_json::from_str(cell_json).map_err(|e| invalid!(CellNotJson, error = e))?;
    let target = value_at_or_insert(&mut root, map)?;
    if target.is_null() {
        *target = serde_json::Value::Object(Default::default());
    }
    let serde_json::Value::Object(object) = target else {
        return Err(invalid!(ValueNotMap));
    };
    let mut entries: Vec<(String, serde_json::Value)> =
        std::mem::take(object).into_iter().collect();
    let gone = || invalid!(EntryGone);
    let taken =
        |entries: &[(String, serde_json::Value)], key: &str, except: Option<usize>| match entries
            .iter()
            .enumerate()
            .any(|(i, (k, _))| k == key && Some(i) != except)
        {
            true => Err(invalid!(KeyExists, key = key)),
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
                Some(json) => serde_json::from_str(&json).map_err(|e| invalid!(Json, error = e))?,
                None => serde_json::Value::Null,
            };
            entries.get_mut(index).ok_or_else(gone)?.1 = value;
        }
        MapEdit::SetKey {
            index,
            key,
            key_type,
        } => {
            let key = map_key(key, key_type)?;
            taken(&entries, &key, Some(index))?;
            entries.get_mut(index).ok_or_else(gone)?.0 = key;
        }
        MapEdit::Insert {
            index,
            key,
            key_type,
        } if index <= entries.len() => {
            let key = map_key(key, key_type)?;
            taken(&entries, &key, None)?;
            entries.insert(index, (key, serde_json::Value::Null));
        }
        MapEdit::Remove(index) if index < entries.len() => {
            entries.remove(index);
        }
        MapEdit::Insert { .. } | MapEdit::Remove(_) => return Err(gone()),
    }
    *object = entries.into_iter().collect();
    Ok(root.to_string())
}

/// A map key typed by the user as the text it has in JSON: as typed for
/// text keys, otherwise read as `key_type` and written back (`01` → `1`).
fn map_key(text: &str, key_type: &DataType) -> Result<String> {
    if is_text(key_type) {
        return Ok(text.to_owned());
    }
    let key = parse_value(Some(text), key_type)?;
    if key.is_null(0) {
        return Err(invalid!(KeyEmpty));
    }
    let key = arrow::compute::cast(&key, &DataType::Utf8)?;
    Ok(key
        .as_any()
        .downcast_ref::<StringArray>()
        .map(|s| s.value(0).to_owned())
        .unwrap_or_default())
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
                        return Err(invalid!(PartGone));
                    }
                }
            }
            _ => value_at(target, std::slice::from_ref(part))?,
        };
    }
    Ok(target)
}

fn value_at<'a>(
    root: &'a mut serde_json::Value,
    pointer: &[crate::display::PathPart],
) -> Result<&'a mut serde_json::Value> {
    use crate::display::PathPart;
    let mut target = root;
    for part in pointer {
        let missing = || invalid!(PartGone);
        target = match part {
            PathPart::Field(name) => target.get_mut(name.as_str()).ok_or_else(missing)?,
            PathPart::Index(i) => target.get_mut(*i).ok_or_else(missing)?,
            PathPart::Entry(i) => match target {
                serde_json::Value::Object(map) => map.values_mut().nth(*i).ok_or_else(missing)?,
                _ => return Err(missing()),
            },
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
mod tests;
