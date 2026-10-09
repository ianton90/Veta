//! Text rendering of values and types, shared by the GUI and the CLI.

use arrow::array::Array;
use arrow::datatypes::{DataType, TimeUnit};
use arrow::record_batch::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};

use crate::error::Result;

/// Formats every cell of `batch` as text, row-major: `result[row][column]`.
/// Nulls become `None`.
pub fn format_batch(batch: &RecordBatch) -> Result<Vec<Vec<Option<String>>>> {
    let options = FormatOptions::default();
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
        | DataType::LargeListView(f)
        | DataType::FixedSizeList(f, _) => format!("list<{}>", type_name(f.data_type())),
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

/// Parses a type name as written by [`type_name`] for flat types, e.g.
/// `int64`, `string`, `timestamp[ms, UTC]`, `decimal(10, 2)`. Also accepts
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
    if let Some(args) = args("decimal", '(', ')') {
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
        assert_eq!(parse_type_name("list<int32>"), None);
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
