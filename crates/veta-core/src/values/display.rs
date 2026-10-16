//! Text rendering of values and types, shared by the GUI and the CLI.

use std::sync::Arc;

use arrow::array::Array;
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};

use crate::error::Result;

/// Formats every cell of `batch` as text, row-major: `result[row][column]`.
/// Nulls become `None`.
pub fn format_batch(batch: &RecordBatch) -> Result<Vec<Vec<Option<String>>>> {
    // Nulls inside nested values are written out.
    let options = FormatOptions::default().with_null("null");
    let formatters = batch
        .columns()
        .iter()
        .map(|column| ArrayFormatter::try_new(column.as_ref(), &options))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok((0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .zip(&formatters)
                .map(|(column, formatter)| {
                    (!column.is_null(row)).then(|| formatter.value(row).to_string())
                })
                .collect()
        })
        .collect())
}

/// One value as JSON text (for nested values: `{"x": 1}`, `[1, 2]`);
/// `None` for null.
pub fn value_json(array: &dyn Array, row: usize) -> Result<Option<String>> {
    if array.is_null(row) {
        return Ok(None);
    }
    let value = with_text_keys(array.slice(row, 1))?;
    let schema = Arc::new(Schema::new(vec![Field::new(
        "v",
        value.data_type().clone(),
        true,
    )]));
    let batch = RecordBatch::try_new(schema, vec![value])?;
    let mut writer = arrow::json::WriterBuilder::new()
        .with_explicit_nulls(true)
        .build::<_, arrow::json::writer::LineDelimited>(Vec::new());
    writer.write(&batch)?;
    writer.finish()?;
    let line = String::from_utf8(writer.into_inner()).unwrap_or_default();
    // `{"v":<value>}` → `<value>`
    let inner = line
        .trim()
        .strip_prefix("{\"v\":")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_default();
    Ok(Some(inner.to_owned()))
}

/// `data_type` with the keys of every map in it turned into text, as JSON
/// needs them (Arrow's JSON reader and writer only take text keys); `None`
/// if it has no maps with other keys.
pub fn json_type(data_type: &DataType) -> Option<DataType> {
    let field = |f: &Arc<Field>| -> Option<Arc<Field>> {
        json_type(f.data_type()).map(|t| Arc::new(f.as_ref().clone().with_data_type(t)))
    };
    match data_type {
        DataType::List(item) => field(item).map(DataType::List),
        DataType::LargeList(item) => field(item).map(DataType::LargeList),
        DataType::FixedSizeList(item, n) => field(item).map(|f| DataType::FixedSizeList(f, *n)),
        DataType::Struct(fields) => {
            let changed: Vec<_> = fields.iter().map(field).collect();
            changed.iter().any(Option::is_some).then(|| {
                let fields: Vec<Arc<Field>> = changed
                    .into_iter()
                    .zip(fields.iter())
                    .map(|(new, old)| new.unwrap_or_else(|| old.clone()))
                    .collect();
                DataType::Struct(fields.into())
            })
        }
        DataType::Map(entries, sorted) => {
            let DataType::Struct(kv) = entries.data_type() else {
                return None;
            };
            let [key, value] = &kv.iter().cloned().collect::<Vec<_>>()[..] else {
                return None;
            };
            let text_key = !crate::values::is_text(key.data_type());
            let new_value = field(value);
            if !text_key && new_value.is_none() {
                return None;
            }
            let key = match text_key {
                true => Arc::new(key.as_ref().clone().with_data_type(DataType::Utf8)),
                false => key.clone(),
            };
            let value = new_value.unwrap_or_else(|| value.clone());
            let entries = entries
                .as_ref()
                .clone()
                .with_data_type(DataType::Struct(vec![key, value].into()));
            Some(DataType::Map(Arc::new(entries), *sorted))
        }
        _ => None,
    }
}

/// `array` with map keys turned into text (see [`json_type`]).
fn with_text_keys(array: arrow::array::ArrayRef) -> Result<arrow::array::ArrayRef> {
    match json_type(array.data_type()) {
        Some(t) => Ok(arrow::compute::cast(&array, &t)?),
        None => Ok(array),
    }
}

/// Every value of `array` as JSON text (nulls stay null), written in one
/// pass. Used to filter and search nested columns.
pub fn json_column(array: &dyn Array) -> Result<arrow::array::StringArray> {
    let column = with_text_keys(arrow::array::make_array(array.to_data()))?;
    let schema = Arc::new(Schema::new(vec![Field::new(
        "v",
        column.data_type().clone(),
        true,
    )]));
    let batch = RecordBatch::try_new(schema, vec![column])?;
    let mut writer = arrow::json::WriterBuilder::new()
        .with_explicit_nulls(true)
        .build::<_, arrow::json::writer::LineDelimited>(Vec::new());
    writer.write(&batch)?;
    writer.finish()?;
    let text = String::from_utf8(writer.into_inner()).unwrap_or_default();
    // One `{"v":<value>}` line per row; JSON escapes newlines in strings.
    Ok(text
        .lines()
        .map(|line| {
            let inner = line
                .strip_prefix("{\"v\":")
                .and_then(|rest| rest.strip_suffix('}'))
                .unwrap_or_default();
            (inner != "null").then(|| inner.to_owned())
        })
        .collect())
}

/// The item range, values and (for maps) keys of a list or map value;
/// `None` for other types and nulls.
pub(crate) fn items_of(
    array: &dyn Array,
    row: usize,
) -> Option<(
    std::ops::Range<usize>,
    arrow::array::ArrayRef,
    Option<arrow::array::ArrayRef>,
)> {
    use arrow::array::AsArray;
    if array.is_null(row) {
        return None;
    }
    match array.data_type() {
        DataType::List(_) => {
            let list = array.as_list::<i32>();
            let o = list.value_offsets();
            Some((
                o[row] as usize..o[row + 1] as usize,
                list.values().clone(),
                None,
            ))
        }
        DataType::LargeList(_) => {
            let list = array.as_list::<i64>();
            let o = list.value_offsets();
            Some((
                o[row] as usize..o[row + 1] as usize,
                list.values().clone(),
                None,
            ))
        }
        DataType::FixedSizeList(_, size) => {
            let list = array.as_fixed_size_list();
            let size = *size as usize;
            Some((row * size..(row + 1) * size, list.values().clone(), None))
        }
        DataType::Map(..) => {
            let map = array.as_map();
            let o = map.value_offsets();
            Some((
                o[row] as usize..o[row + 1] as usize,
                map.values().clone(),
                Some(map.keys().clone()),
            ))
        }
        _ => None,
    }
}

/// Whether values of `data_type` are lists or maps.
pub fn has_items(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(..)
            | DataType::Map(..)
    )
}

/// A short summary of a list or map value: `[3 items]`, `{2 entries}`,
/// `[]`; `None` for other types and nulls.
pub fn items_summary(array: &dyn Array, row: usize) -> Option<String> {
    let (range, _, keys) = items_of(array, row)?;
    Some(match (range.len(), keys.is_some()) {
        (0, false) => "[]".into(),
        (0, true) => "{}".into(),
        (1, false) => "[1 item]".into(),
        (1, true) => "{1 entry}".into(),
        (n, false) => format!("[{n} items]"),
        (n, true) => format!("{{{n} entries}}"),
    })
}

/// The field at `path` (field names) of a struct `array`, null wherever a
/// parent struct is; `array` itself for an empty path. `None` if a step of
/// the path isn't a struct field.
pub fn field_array(
    array: &arrow::array::ArrayRef,
    path: &[String],
) -> Option<arrow::array::ArrayRef> {
    use arrow::array::{AsArray, make_array};
    let mut current = array.clone();
    for name in path {
        if !matches!(current.data_type(), DataType::Struct(_)) {
            return None;
        }
        let parent = current.as_struct();
        let child = parent.column_by_name(name)?;
        let nulls = arrow::buffer::NullBuffer::union(parent.nulls(), child.nulls());
        let data = child.to_data().into_builder().nulls(nulls).build().ok()?;
        current = make_array(data);
    }
    Some(current)
}

/// The type of the field at `path` inside `data_type`.
pub fn field_type<'a>(data_type: &'a DataType, path: &[String]) -> Option<&'a DataType> {
    let mut current = data_type;
    for name in path {
        let DataType::Struct(fields) = current else {
            return None;
        };
        current = fields.iter().find(|f| f.name() == name)?.data_type();
    }
    Some(current)
}

/// Number of items in a list or map value (0 for other types and nulls).
pub fn item_count(array: &dyn Array, row: usize) -> usize {
    items_of(array, row).map_or(0, |(range, _, _)| range.len())
}

/// A step into a nested value: a struct field, a list item or the value of
/// a map entry, by position.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathPart {
    Field(String),
    Index(usize),
    Entry(usize),
}

/// The type of the value at `path` inside a value of `data_type`.
pub fn type_at<'a>(data_type: &'a DataType, path: &[PathPart]) -> Option<&'a DataType> {
    let mut current = data_type;
    for part in path {
        current = match (current, part) {
            (DataType::Struct(fields), PathPart::Field(name)) => {
                fields.iter().find(|f| f.name() == name)?.data_type()
            }
            (
                DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _),
                PathPart::Index(_),
            ) => item.data_type(),
            (DataType::Map(entries, _), PathPart::Entry(_)) => match entries.data_type() {
                DataType::Struct(kv) if kv.len() == 2 => kv[1].data_type(),
                _ => return None,
            },
            _ => return None,
        };
    }
    Some(current)
}

/// The key type of a map type.
pub fn map_key_type(data_type: &DataType) -> Option<&DataType> {
    match data_type {
        DataType::Map(entries, _) => match entries.data_type() {
            DataType::Struct(kv) if kv.len() == 2 => Some(kv[0].data_type()),
            _ => None,
        },
        _ => None,
    }
}

/// Short, human-friendly name of a type, e.g. `int64`, `string`,
/// `timestamp[ms, UTC]`, `list<int32>`.
pub fn type_name(data_type: &DataType) -> String {
    match data_type {
        DataType::Null => "null".into(),
        DataType::Boolean => "bool".into(),
        DataType::Int8 => "int8".into(),
        DataType::Int16 => "int16".into(),
        DataType::Int32 => "int32".into(),
        DataType::Int64 => "int64".into(),
        DataType::UInt8 => "uint8".into(),
        DataType::UInt16 => "uint16".into(),
        DataType::UInt32 => "uint32".into(),
        DataType::UInt64 => "uint64".into(),
        DataType::Float16 => "float16".into(),
        DataType::Float32 => "float32".into(),
        DataType::Float64 => "float64".into(),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "string".into(),
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView => "binary".into(),
        DataType::FixedSizeBinary(n) => format!("binary[{n}]"),
        DataType::Date32 | DataType::Date64 => "date".into(),
        DataType::Time32(unit) | DataType::Time64(unit) => format!("time[{}]", unit_name(*unit)),
        DataType::Timestamp(unit, None) => format!("timestamp[{}]", unit_name(*unit)),
        DataType::Timestamp(unit, Some(tz)) => format!("timestamp[{}, {tz}]", unit_name(*unit)),
        DataType::Duration(unit) => format!("duration[{}]", unit_name(*unit)),
        DataType::Interval(_) => "interval".into(),
        DataType::Decimal32(p, s)
        | DataType::Decimal64(p, s)
        | DataType::Decimal128(p, s)
        | DataType::Decimal256(p, s) => format!("decimal({p}, {s})"),
        DataType::List(f)
        | DataType::LargeList(f)
        | DataType::ListView(f)
        | DataType::LargeListView(f) => format!("list<{}>", type_name(f.data_type())),
        DataType::FixedSizeList(f, n) => {
            format!("fixed_size_list<{}, {n}>", type_name(f.data_type()))
        }
        DataType::Struct(fields) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name(), type_name(f.data_type())))
                .collect();
            format!("struct<{}>", inner.join(", "))
        }
        DataType::Map(entries, _) => match entries.data_type() {
            DataType::Struct(kv) if kv.len() == 2 => format!(
                "map<{}, {}>",
                type_name(kv[0].data_type()),
                type_name(kv[1].data_type())
            ),
            other => format!("map<{}>", type_name(other)),
        },
        DataType::Dictionary(_, value) => type_name(value),
        DataType::RunEndEncoded(_, values) => type_name(values.data_type()),
        DataType::Union(..) => "union".into(),
    }
}

/// A map type with `key` keys and `value` values (nullable), named as
/// Arrow names them.
pub fn map_type(key: DataType, value: DataType) -> DataType {
    let entries = DataType::Struct(
        vec![
            Field::new("keys", key, false),
            Field::new("values", value, true),
        ]
        .into(),
    );
    DataType::Map(Arc::new(Field::new("entries", entries, false)), false)
}

/// Parses a type name as written by [`type_name`], e.g. `int64`, `string`,
/// `timestamp[ms, UTC]`, `decimal(10, 2)`, `list<int64>`,
/// `map<string, int64>`. Also accepts
/// a few aliases (`int`, `float`, `double`, `text`, `boolean`).
pub fn parse_type_name(name: &str) -> Option<DataType> {
    let raw = name.trim();
    let lower = raw.to_ascii_lowercase();
    let unit = |u: &str| match u.trim() {
        "s" => Some(TimeUnit::Second),
        "ms" => Some(TimeUnit::Millisecond),
        "us" => Some(TimeUnit::Microsecond),
        "ns" => Some(TimeUnit::Nanosecond),
        _ => None,
    };
    // Arguments inside brackets, from the original text (time zone names are
    // case-sensitive).
    let args = |prefix: &str, open: char, close: char| -> Option<&str> {
        if !lower.starts_with(prefix) {
            return None;
        }
        raw[prefix.len()..].strip_prefix(open)?.strip_suffix(close)
    };
    let simple = match lower.as_str() {
        "bool" | "boolean" => Some(DataType::Boolean),
        "int8" => Some(DataType::Int8),
        "int16" => Some(DataType::Int16),
        "int32" => Some(DataType::Int32),
        "int64" | "int" | "integer" => Some(DataType::Int64),
        "uint8" => Some(DataType::UInt8),
        "uint16" => Some(DataType::UInt16),
        "uint32" => Some(DataType::UInt32),
        "uint64" => Some(DataType::UInt64),
        "float32" | "float" => Some(DataType::Float32),
        "float64" | "double" => Some(DataType::Float64),
        "string" | "text" | "utf8" => Some(DataType::Utf8),
        "binary" => Some(DataType::Binary),
        "date" => Some(DataType::Date32),
        "timestamp" => Some(DataType::Timestamp(TimeUnit::Microsecond, None)),
        _ => None,
    };
    if simple.is_some() {
        return simple;
    }
    if let Some(args) = args("timestamp", '[', ']') {
        let (u, tz) = match args.split_once(',') {
            Some((u, tz)) => (u, Some(tz.trim()).filter(|t| !t.is_empty())),
            None => (args, None),
        };
        return Some(DataType::Timestamp(
            unit(&u.to_ascii_lowercase())?,
            tz.map(Into::into),
        ));
    }
    if let Some(args) = args("time", '[', ']') {
        return Some(match unit(&args.to_ascii_lowercase())? {
            u @ (TimeUnit::Second | TimeUnit::Millisecond) => DataType::Time32(u),
            u => DataType::Time64(u),
        });
    }
    if let Some(args) = args("duration", '[', ']') {
        return Some(DataType::Duration(unit(&args.to_ascii_lowercase())?));
    }
    if let Some(inner) = args("list", '<', '>') {
        let item = parse_type_name(inner)?;
        return Some(DataType::List(Arc::new(Field::new("item", item, true))));
    }
    if let Some(inner) = args("fixed_size_list", '<', '>') {
        let [item, size] = split_top_level(inner)[..] else {
            return None;
        };
        let size: i32 = size.trim().parse().ok().filter(|n| *n > 0)?;
        let item = parse_type_name(item)?;
        return Some(DataType::FixedSizeList(
            Arc::new(Field::new("item", item, true)),
            size,
        ));
    }
    if let Some(inner) = args("map", '<', '>') {
        let [key, value] = split_top_level(inner)[..] else {
            return None;
        };
        return Some(map_type(parse_type_name(key)?, parse_type_name(value)?));
    }
    if let Some(inner) = args("struct", '<', '>') {
        let fields = split_top_level(inner)
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .map(|part| {
                let (name, t) = part.split_once(':')?;
                Some(Field::new(name.trim(), parse_type_name(t)?, true))
            })
            .collect::<Option<Vec<_>>>()?;
        return Some(DataType::Struct(fields.into()));
    }
    let decimal = args("decimal128", '(', ')')
        .or_else(|| args("decimal256", '(', ')'))
        .or_else(|| args("decimal", '(', ')'));
    if let Some(args) = decimal {
        let (p, s) = args.split_once(',')?;
        let precision: u8 = p.trim().parse().ok()?;
        let scale: i8 = s.trim().parse().ok()?;
        return Some(if precision <= 38 {
            DataType::Decimal128(precision, scale)
        } else {
            DataType::Decimal256(precision, scale)
        });
    }
    None
}

/// `text` split at commas that aren't inside `<>`, `()` or `[]`.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Whether values of this type read best right-aligned.
pub fn is_numeric(data_type: &DataType) -> bool {
    data_type.is_numeric() || matches!(data_type, DataType::Dictionary(_, v) if v.is_numeric())
}

/// Shortens `value` to at most `max` characters for display, noting how much
/// was cut, e.g. `abcdef… (1.2 KiB)`.
pub fn abbreviate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let head: String = value.chars().take(max).collect();
    format!("{head}… ({})", human_bytes(value.len() as u64))
}

/// Formats a byte count with binary units, e.g. `1.5 MiB`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn unit_name(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Second => "s",
        TimeUnit::Millisecond => "ms",
        TimeUnit::Microsecond => "us",
        TimeUnit::Nanosecond => "ns",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int32Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use std::sync::Arc;

    #[test]
    fn formats_values_and_nulls() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, true),
            Field::new("b", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![Some(1), None])),
                Arc::new(StringArray::from(vec![None, Some("x")])),
            ],
        )
        .unwrap();
        let cells = format_batch(&batch).unwrap();
        assert_eq!(
            cells,
            vec![
                vec![Some("1".to_owned()), None],
                vec![None, Some("x".to_owned())]
            ]
        );
    }

    #[test]
    fn list_and_map_items() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
        let batch = doc.read(0..4).unwrap();
        let (tags, scores) = (batch.column(2), batch.column(4));
        assert_eq!(
            items_summary(tags.as_ref(), 0).as_deref(),
            Some("[2 items]")
        );
        assert_eq!(items_summary(tags.as_ref(), 1).as_deref(), Some("[]"));
        assert_eq!(items_summary(tags.as_ref(), 2), None, "null");
        assert_eq!(
            items_summary(scores.as_ref(), 0).as_deref(),
            Some("{2 entries}")
        );
        assert_eq!(item_count(tags.as_ref(), 3), 3);
        assert!(!has_items(&DataType::Int64));
        // Struct type names parse back, nested too.
        for field in batch.schema().fields() {
            let name = type_name(field.data_type());
            if name.starts_with("map") {
                continue;
            }
            assert_eq!(
                parse_type_name(&name).as_ref(),
                Some(field.data_type()),
                "{name}"
            );
        }
    }

    #[test]
    fn type_names() {
        assert_eq!(type_name(&DataType::Int64), "int64");
        assert_eq!(
            type_name(&DataType::Timestamp(
                TimeUnit::Millisecond,
                Some("UTC".into())
            )),
            "timestamp[ms, UTC]"
        );
        let list = DataType::List(Arc::new(Field::new("item", DataType::Int32, true)));
        assert_eq!(type_name(&list), "list<int32>");
    }

    #[test]
    fn abbreviates_long_values() {
        assert_eq!(abbreviate("short", 10), "short");
        assert_eq!(abbreviate(&"x".repeat(2048), 3), "xxx… (2.0 KiB)");
    }

    #[test]
    fn parses_type_names() {
        for data_type in [
            DataType::Int64,
            DataType::Utf8,
            DataType::Boolean,
            DataType::Float32,
            DataType::Date32,
            DataType::Time64(TimeUnit::Microsecond),
            DataType::Timestamp(TimeUnit::Millisecond, None),
            DataType::Timestamp(TimeUnit::Nanosecond, Some("Europe/Madrid".into())),
            DataType::Decimal128(10, 2),
        ] {
            assert_eq!(parse_type_name(&type_name(&data_type)), Some(data_type));
        }
        assert_eq!(parse_type_name(" Double "), Some(DataType::Float64));
        assert_eq!(parse_type_name("timestamp[xs]"), None);
        let scores = map_type(DataType::Utf8, DataType::Int64);
        assert_eq!(parse_type_name("map<string, int64>"), Some(scores.clone()));
        assert_eq!(parse_type_name(&type_name(&scores)), Some(scores));
        assert_eq!(parse_type_name("map<string>"), None);
        let xyz = DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float64, true)), 3);
        assert_eq!(type_name(&xyz), "fixed_size_list<float64, 3>");
        assert_eq!(parse_type_name(&type_name(&xyz)), Some(xyz));
        assert_eq!(parse_type_name("fixed_size_list<int64, 0>"), None);
        assert_eq!(
            parse_type_name("list<int32>"),
            Some(DataType::List(Arc::new(Field::new(
                "item",
                DataType::Int32,
                true
            ))))
        );
    }

    #[test]
    fn bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use crate::{Document, OpenOptions};
    use veta_testkit::{TempDir, fixtures};

    #[test]
    fn formats_every_fixture_type() {
        let dir = TempDir::new();
        let path = dir.join("all.parquet");
        fixtures::all_types(&path);
        let doc = Document::open(&path, OpenOptions::default()).unwrap();
        let cells = format_batch(&doc.read(0..fixtures::ALL_TYPES_ROWS).unwrap()).unwrap();
        assert_eq!(cells.len(), fixtures::ALL_TYPES_ROWS);
        assert!(cells[2].iter().all(Option::is_none), "row 2 is all nulls");

        let schema = doc.schema();
        let col = |name: &str| schema.index_of(name).unwrap();
        assert_eq!(cells[0][col("utf8")].as_deref(), Some("alpha"));
        assert_eq!(cells[1][col("decimal")].as_deref(), Some("0.00"));
        assert_eq!(cells[0][col("decimal")].as_deref(), Some("123.45"));
        assert_eq!(cells[0][col("date32")].as_deref(), Some("1970-01-01"));
        assert!(
            cells[1][col("timestamp_us_utc")]
                .as_deref()
                .unwrap()
                .starts_with("2023-11-14T22:13:20")
        );
    }
}

#[cfg(test)]
mod nested_tests {
    use super::*;

    #[test]
    fn nested_cells_format() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
        let batch = doc.read(0..4).unwrap();
        let cells = format_batch(&batch).unwrap();
        assert_eq!(cells[1][1].as_deref(), Some("{x: -1, y: null}"));
    }
}
