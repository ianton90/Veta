//! Parquet fixture generators.
//!
//! | Fixture | Covers |
//! |---|---|
//! | [`all_types`] | every primitive type we support, with nulls |
//! | [`mixed_settings`] | several row groups, per-column compression, encoding, dictionary and statistics settings, key/value metadata |
//! | [`large`] | many rows and row groups, for paging tests |
//! | [`nested`] | struct, list, list of structs, map and struct-in-struct columns |
//! | [`deep_nested`] | many rows of structs, lists and maps nested inside each other, with nulls and empties |

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array,
    Int8Array, Int16Array, Int32Array, Int64Array, LargeStringArray, StringArray,
    Time64MicrosecondArray, TimestampMicrosecondArray, TimestampMillisecondArray, UInt8Array,
    UInt16Array, UInt32Array, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, GzipLevel, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::schema::types::ColumnPath;

/// Rows in [`all_types`].
pub const ALL_TYPES_ROWS: usize = 4;

/// Writes one row group with a column per primitive type. Row 2 is null in
/// every column; the other rows include edge values (min/max, empty string).
pub fn all_types(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("bool", DataType::Boolean, true),
        Field::new("i8", DataType::Int8, true),
        Field::new("i16", DataType::Int16, true),
        Field::new("i32", DataType::Int32, true),
        Field::new("i64", DataType::Int64, true),
        Field::new("u8", DataType::UInt8, true),
        Field::new("u16", DataType::UInt16, true),
        Field::new("u32", DataType::UInt32, true),
        Field::new("u64", DataType::UInt64, true),
        Field::new("f32", DataType::Float32, true),
        Field::new("f64", DataType::Float64, true),
        Field::new("utf8", DataType::Utf8, true),
        Field::new("large_utf8", DataType::LargeUtf8, true),
        Field::new("binary", DataType::Binary, true),
        Field::new("date32", DataType::Date32, true),
        Field::new("time64_us", DataType::Time64(TimeUnit::Microsecond), true),
        Field::new(
            "timestamp_ms",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            true,
        ),
        Field::new(
            "timestamp_us_utc",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new("decimal", DataType::Decimal128(10, 2), true),
    ]));

    let columns: Vec<ArrayRef> = vec![
        Arc::new(BooleanArray::from(vec![
            Some(true),
            Some(false),
            None,
            Some(true),
        ])),
        Arc::new(Int8Array::from(vec![
            Some(i8::MIN),
            Some(0),
            None,
            Some(i8::MAX),
        ])),
        Arc::new(Int16Array::from(vec![
            Some(i16::MIN),
            Some(0),
            None,
            Some(i16::MAX),
        ])),
        Arc::new(Int32Array::from(vec![
            Some(i32::MIN),
            Some(0),
            None,
            Some(i32::MAX),
        ])),
        Arc::new(Int64Array::from(vec![
            Some(i64::MIN),
            Some(0),
            None,
            Some(i64::MAX),
        ])),
        Arc::new(UInt8Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u8::MAX),
        ])),
        Arc::new(UInt16Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u16::MAX),
        ])),
        Arc::new(UInt32Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u32::MAX),
        ])),
        Arc::new(UInt64Array::from(vec![
            Some(0),
            Some(1),
            None,
            Some(u64::MAX),
        ])),
        Arc::new(Float32Array::from(vec![
            Some(-1.5),
            Some(0.0),
            None,
            Some(f32::MAX),
        ])),
        Arc::new(Float64Array::from(vec![
            Some(-1.5),
            Some(0.1),
            None,
            Some(f64::MAX),
        ])),
        Arc::new(StringArray::from(vec![
            Some("alpha"),
            Some(""),
            None,
            Some("ñandú ✓"),
        ])),
        Arc::new(LargeStringArray::from(vec![
            Some("x"),
            Some("yy"),
            None,
            Some("zzz"),
        ])),
        Arc::new(BinaryArray::from(vec![
            Some(&b"\x00\x01"[..]),
            Some(&b""[..]),
            None,
            Some(&b"\xff"[..]),
        ])),
        Arc::new(Date32Array::from(vec![
            Some(0),
            Some(19_000),
            None,
            Some(-1),
        ])),
        Arc::new(Time64MicrosecondArray::from(vec![
            Some(0),
            Some(43_200_000_000),
            None,
            Some(86_399_999_999),
        ])),
        Arc::new(TimestampMillisecondArray::from(vec![
            Some(0),
            Some(1_700_000_000_000),
            None,
            Some(-1),
        ])),
        Arc::new(
            TimestampMicrosecondArray::from(vec![
                Some(0),
                Some(1_700_000_000_000_000),
                None,
                Some(1),
            ])
            .with_timezone("UTC"),
        ),
        Arc::new(
            Decimal128Array::from(vec![Some(12_345), Some(0), None, Some(-99_999_999)])
                .with_precision_and_scale(10, 2)
                .unwrap(),
        ),
    ];

    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
    write(path, &[batch], WriterProperties::default());
}

/// Rows in [`mixed_settings`].
pub const MIXED_SETTINGS_ROWS: usize = 1_000;
/// Maximum rows per row group in [`mixed_settings`].
pub const MIXED_SETTINGS_ROW_GROUP: usize = 300;
/// Key/value metadata written by [`mixed_settings`].
pub const MIXED_SETTINGS_METADATA: &[(&str, &str)] = &[
    ("owner", "veta-tests"),
    ("purpose", "writer settings round trip"),
];

/// Writes [`MIXED_SETTINGS_ROWS`] rows in row groups of
/// [`MIXED_SETTINGS_ROW_GROUP`] with different settings per column:
///
/// | Column | Compression | Encoding | Dictionary | Statistics |
/// |---|---|---|---|---|
/// | `id` (i64) | zstd(3) | delta binary packed | off | page |
/// | `name` (utf8) | snappy | plain | on | chunk |
/// | `score` (f64) | gzip(6) | byte stream split | off | none |
/// | `flag` (bool) | uncompressed | plain | off | chunk |
pub fn mixed_settings(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
        Field::new("flag", DataType::Boolean, true),
    ]));

    let n = MIXED_SETTINGS_ROWS;
    let names = ["ana", "bo", "cy", "dee", "eli"];
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from_iter_values(0..n as i64)),
        Arc::new(StringArray::from_iter(
            (0..n).map(|i| (i % 7 != 0).then(|| names[i % names.len()])),
        )),
        Arc::new(Float64Array::from_iter(
            (0..n).map(|i| (i % 11 != 0).then_some(i as f64 * 0.5)),
        )),
        Arc::new(BooleanArray::from_iter((0..n).map(|i| Some(i % 2 == 0)))),
    ];
    let batch = RecordBatch::try_new(schema, columns).unwrap();

    let col = |name: &str| ColumnPath::from(name);
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(MIXED_SETTINGS_ROW_GROUP))
        .set_key_value_metadata(Some(
            MIXED_SETTINGS_METADATA
                .iter()
                .map(|(k, v)| KeyValue::new((*k).into(), (*v).to_owned()))
                .collect(),
        ))
        // id
        .set_column_compression(col("id"), Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
        .set_column_dictionary_enabled(col("id"), false)
        .set_column_encoding(col("id"), Encoding::DELTA_BINARY_PACKED)
        .set_column_statistics_enabled(col("id"), EnabledStatistics::Page)
        // name
        .set_column_compression(col("name"), Compression::SNAPPY)
        .set_column_dictionary_enabled(col("name"), true)
        .set_column_statistics_enabled(col("name"), EnabledStatistics::Chunk)
        // score
        .set_column_compression(
            col("score"),
            Compression::GZIP(GzipLevel::try_new(6).unwrap()),
        )
        .set_column_dictionary_enabled(col("score"), false)
        .set_column_encoding(col("score"), Encoding::BYTE_STREAM_SPLIT)
        .set_column_statistics_enabled(col("score"), EnabledStatistics::None)
        // flag
        .set_column_compression(col("flag"), Compression::UNCOMPRESSED)
        .set_column_dictionary_enabled(col("flag"), false)
        .set_column_statistics_enabled(col("flag"), EnabledStatistics::Chunk)
        .build();

    write(path, &[batch], props);
}

/// Writes `rows` rows of `(id: i64, value: f64, label: utf8)` in row groups of
/// `row_group_size`, generated in batches so memory use stays bounded.
pub fn large(path: &Path, rows: usize, row_group_size: usize) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("value", DataType::Float64, false),
        Field::new("label", DataType::Utf8, false),
    ]));
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(row_group_size))
        .build();

    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props)).unwrap();
    const BATCH: usize = 64 * 1024;
    let mut start = 0;
    while start < rows {
        let end = (start + BATCH).min(rows);
        let ids = start as i64..end as i64;
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from_iter_values(ids.clone())),
                Arc::new(Float64Array::from_iter_values(
                    ids.clone().map(|i| i as f64 / 3.0),
                )),
                Arc::new(StringArray::from_iter_values(
                    ids.map(|i| format!("row-{i}")),
                )),
            ],
        )
        .unwrap();
        writer.write(&batch).unwrap();
        start = end;
    }
    writer.close().unwrap();
}

fn write(path: &Path, batches: &[RecordBatch], props: WriterProperties) {
    let file = File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batches[0].schema(), Some(props)).unwrap();
    for batch in batches {
        writer.write(batch).unwrap();
    }
    writer.close().unwrap();
}

/// Rows in [`nested`].
pub const NESTED_ROWS: usize = 4;

/// Schema of [`nested`].
pub fn nested_schema() -> Arc<Schema> {
    let point = DataType::Struct(
        vec![
            Field::new("x", DataType::Int64, true),
            Field::new("y", DataType::Int64, true),
        ]
        .into(),
    );
    let map = DataType::Map(
        Arc::new(Field::new(
            "entries",
            DataType::Struct(
                vec![
                    Field::new("keys", DataType::Utf8, false),
                    Field::new("values", DataType::Int64, true),
                ]
                .into(),
            ),
            false,
        )),
        false,
    );
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("point", point.clone(), true),
        Field::new("tags", DataType::new_list(DataType::Utf8, true), true),
        Field::new("path", DataType::new_list(point.clone(), true), true),
        Field::new("scores", map, true),
        Field::new(
            "owner",
            DataType::Struct(
                vec![
                    Field::new("name", DataType::Utf8, true),
                    Field::new("home", point, true),
                ]
                .into(),
            ),
            true,
        ),
    ]))
}

/// Writes [`NESTED_ROWS`] rows of nested columns. Row 2 is null (or empty)
/// in every nested column.
pub fn nested(path: &Path) {
    let json = r#"
{"id": 0, "point": {"x": 1, "y": 2}, "tags": ["a", "b"], "path": [{"x": 0, "y": 0}, {"x": 3, "y": 4}], "scores": {"math": 9, "art": 7}, "owner": {"name": "Ana", "home": {"x": 5, "y": 6}}}
{"id": 1, "point": {"x": -1, "y": null}, "tags": [], "path": [{"x": 1, "y": 1}], "scores": {}, "owner": {"name": "Bo", "home": null}}
{"id": 2, "point": null, "tags": null, "path": null, "scores": null, "owner": null}
{"id": 3, "point": {"x": 7, "y": 8}, "tags": ["c", null, "d"], "path": [], "scores": {"math": null}, "owner": {"name": null, "home": {"x": 0, "y": 0}}}
"#;
    let schema = nested_schema();
    let mut reader = arrow::json::ReaderBuilder::new(schema.clone())
        .build(std::io::Cursor::new(json))
        .unwrap();
    let batch = reader.next().unwrap().unwrap();
    write(path, &[batch], WriterProperties::builder().build());
}

/// A map type named as Arrow names maps.
fn map_of(key: DataType, value: DataType) -> DataType {
    let entries = DataType::Struct(
        vec![
            Field::new("keys", key, false),
            Field::new("values", value, true),
        ]
        .into(),
    );
    DataType::Map(Arc::new(Field::new("entries", entries, false)), false)
}

fn list_of(item: DataType) -> DataType {
    DataType::new_list(item, true)
}

fn struct_of(fields: Vec<(&str, DataType)>) -> DataType {
    DataType::Struct(
        fields
            .into_iter()
            .map(|(name, t)| Field::new(name, t, true))
            .collect::<Vec<_>>()
            .into(),
    )
}

/// Schema of [`deep_nested`]. `keys` says whether map keys are text; the
/// `history` map has whole-number keys, written through text keys first.
fn deep_nested_schema_with(int_keys: bool) -> Arc<Schema> {
    let geo = struct_of(vec![("lat", DataType::Float64), ("lon", DataType::Float64)]);
    let address = struct_of(vec![
        ("street", DataType::Utf8),
        ("city", DataType::Utf8),
        ("geo", geo),
    ]);
    let order = struct_of(vec![
        ("sku", DataType::Utf8),
        ("qty", DataType::Int64),
        ("price", DataType::Float64),
        ("options", map_of(DataType::Utf8, DataType::Utf8)),
    ]);
    let event = struct_of(vec![
        ("status", DataType::Utf8),
        ("notes", list_of(DataType::Utf8)),
    ]);
    let history_key = if int_keys {
        DataType::Int64
    } else {
        DataType::Utf8
    };
    let child = struct_of(vec![("name", DataType::Utf8), ("age", DataType::Int64)]);
    let profile = struct_of(vec![
        ("prefs", map_of(DataType::Utf8, DataType::Boolean)),
        ("emails", list_of(DataType::Utf8)),
        ("children", list_of(child)),
    ]);
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("address", address, true),
        Field::new("tags", list_of(DataType::Utf8), true),
        Field::new("scores", map_of(DataType::Utf8, DataType::Int64), true),
        Field::new("matrix", list_of(list_of(DataType::Int64)), true),
        Field::new("orders", list_of(order), true),
        Field::new(
            "attrs",
            map_of(DataType::Utf8, list_of(DataType::Utf8)),
            true,
        ),
        Field::new("history", map_of(history_key, event), true),
        Field::new("profile", profile, true),
    ]))
}

/// Schema of [`deep_nested`]: structs in structs, lists of lists, lists of
/// structs holding maps, maps of lists, a whole-number-keyed map of structs
/// holding lists, and a struct of a map, a list and a list of structs.
pub fn deep_nested_schema() -> Arc<Schema> {
    deep_nested_schema_with(true)
}

/// Writes `rows` rows of [`deep_nested_schema`] in row groups of 100, made
/// up deterministically. Some values are null and some lists and maps empty,
/// in every nested column and at every level.
pub fn deep_nested(path: &Path, rows: usize) {
    // splitmix64: the same file on every run.
    let state = std::cell::Cell::new(42_u64);
    let next = |n: u64| {
        let x = state.get().wrapping_add(0x9e37_79b9_7f4a_7c15);
        state.set(x);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) % n.max(1)
    };
    const NAMES: [&str; 12] = [
        "Ana", "Bo", "Chen", "Dara", "Eli", "Fatou", "Goran", "Hana", "Ivo", "Jun", "Kemal", "Lea",
    ];
    const CITIES: [&str; 8] = [
        "Lisbon", "Oslo", "Lima", "Kyoto", "Accra", "Quito", "Riga", "Perth",
    ];
    const STREETS: [&str; 6] = [
        "Main St",
        "Oak Ave",
        "Rue Verte",
        "Calle Sol",
        "Ring 3",
        "Hill Rd",
    ];
    const TAGS: [&str; 8] = ["new", "vip", "late", "gift", "eu", "promo", "bulk", "test"];
    const SUBJECTS: [&str; 6] = ["math", "art", "music", "pe", "history", "code"];
    const STATUSES: [&str; 4] = ["created", "paid", "shipped", "returned"];
    const COLORS: [&str; 5] = ["red", "blue", "green", "black", "white"];
    let quote = |s: &str| format!("\"{s}\"");
    let join = |items: Vec<String>| items.join(", ");
    let mut lines = Vec::with_capacity(rows);
    for id in 0..rows {
        // Mostly values; sometimes null (0) or empty (1).
        let kind = || next(12);
        let name = match kind() {
            0 => "null".into(),
            _ => quote(NAMES[next(12) as usize]),
        };
        let address = match kind() {
            0 => "null".into(),
            _ => {
                let geo = match next(5) {
                    0 => "null".into(),
                    _ => format!(
                        "{{\"lat\": {:.4}, \"lon\": {:.4}}}",
                        next(18_000) as f64 / 100.0 - 90.0,
                        next(36_000) as f64 / 100.0 - 180.0
                    ),
                };
                let street = match next(6) {
                    0 => "null".into(),
                    _ => quote(&format!("{} {}", next(200) + 1, STREETS[next(6) as usize])),
                };
                format!(
                    "{{\"street\": {street}, \"city\": {}, \"geo\": {geo}}}",
                    quote(CITIES[next(8) as usize])
                )
            }
        };
        let list = |n: u64, item: &mut dyn FnMut() -> String| -> String {
            format!("[{}]", join((0..n).map(|_| item()).collect()))
        };
        let tags = match kind() {
            0 => "null".into(),
            1 => "[]".into(),
            _ => {
                let n = next(4) + 1;
                list(n, &mut || match next(10) {
                    0 => "null".into(),
                    _ => quote(TAGS[next(8) as usize]),
                })
            }
        };
        let scores = match kind() {
            0 => "null".into(),
            1 => "{}".into(),
            _ => {
                let mut used: Vec<&str> = SUBJECTS.to_vec();
                let n = next(4) + 1;
                let entries = (0..n)
                    .map(|_| {
                        let key = used.remove(next(used.len() as u64) as usize);
                        let value = match next(8) {
                            0 => "null".into(),
                            _ => (next(10) + 1).to_string(),
                        };
                        format!("\"{key}\": {value}")
                    })
                    .collect();
                format!("{{{}}}", join(entries))
            }
        };
        let matrix = match kind() {
            0 => "null".into(),
            1 => "[]".into(),
            _ => {
                let rows_n = next(3) + 1;
                let cols = next(4);
                let inner: Vec<String> = (0..rows_n)
                    .map(|_| match next(8) {
                        0 => "null".into(),
                        _ => format!(
                            "[{}]",
                            join(
                                (0..cols)
                                    .map(|_| (next(100) as i64 - 20).to_string())
                                    .collect()
                            )
                        ),
                    })
                    .collect();
                format!("[{}]", join(inner))
            }
        };
        let orders = match kind() {
            0 => "null".into(),
            1 => "[]".into(),
            _ => {
                let n = next(3) + 1;
                let items: Vec<String> = (0..n)
                    .map(|_| {
                        let options = match next(4) {
                            0 => "null".into(),
                            1 => "{}".into(),
                            _ => format!(
                                "{{\"color\": \"{}\", \"size\": \"{}\"}}",
                                COLORS[next(5) as usize],
                                ["S", "M", "L", "XL"][next(4) as usize]
                            ),
                        };
                        format!(
                            "{{\"sku\": \"SKU-{:04}\", \"qty\": {}, \"price\": {:.2}, \"options\": {options}}}",
                            next(10_000),
                            next(5) + 1,
                            next(20_000) as f64 / 100.0
                        )
                    })
                    .collect();
                format!("[{}]", join(items))
            }
        };
        let attrs = match kind() {
            0 => "null".into(),
            1 => "{}".into(),
            _ => {
                let mut keys = vec!["colors", "sizes", "labels"];
                let n = next(3) + 1;
                let entries: Vec<String> = (0..n)
                    .map(|_| {
                        let key = keys.remove(next(keys.len() as u64) as usize);
                        let value = match next(6) {
                            0 => "null".into(),
                            1 => "[]".into(),
                            _ => {
                                let m = next(3) + 1;
                                format!(
                                    "[{}]",
                                    join((0..m).map(|_| quote(COLORS[next(5) as usize])).collect())
                                )
                            }
                        };
                        format!("\"{key}\": {value}")
                    })
                    .collect();
                format!("{{{}}}", join(entries))
            }
        };
        let history = match kind() {
            0 => "null".into(),
            1 => "{}".into(),
            _ => {
                let n = next(3) + 1;
                let start = 20_240_000 + next(10_000);
                let entries: Vec<String> = (0..n)
                    .map(|i| {
                        let notes = match next(4) {
                            0 => "null".into(),
                            1 => "[]".into(),
                            _ => format!("[\"note {}\"]", next(100)),
                        };
                        let event = match next(10) {
                            0 => "null".into(),
                            _ => format!(
                                "{{\"status\": \"{}\", \"notes\": {notes}}}",
                                STATUSES[(i as usize + next(2) as usize) % 4]
                            ),
                        };
                        format!("\"{}\": {event}", start + i * 7)
                    })
                    .collect();
                format!("{{{}}}", join(entries))
            }
        };
        let profile = match kind() {
            0 => "null".into(),
            _ => {
                let prefs = match next(5) {
                    0 => "null".into(),
                    1 => "{}".into(),
                    _ => format!(
                        "{{\"newsletter\": {}, \"dark_mode\": {}}}",
                        next(2) == 0,
                        next(2) == 0
                    ),
                };
                let first = NAMES[id % 12].to_lowercase();
                let emails = match next(5) {
                    0 => "null".into(),
                    1 => "[]".into(),
                    _ => format!("[\"{first}@example.com\", \"{first}{id}@mail.test\"]"),
                };
                let children = match next(4) {
                    0 => "null".into(),
                    1 => "[]".into(),
                    _ => {
                        let n = next(3) + 1;
                        let kids: Vec<String> = (0..n)
                            .map(|_| {
                                format!(
                                    "{{\"name\": \"{}\", \"age\": {}}}",
                                    NAMES[next(12) as usize],
                                    next(18)
                                )
                            })
                            .collect();
                        format!("[{}]", join(kids))
                    }
                };
                format!("{{\"prefs\": {prefs}, \"emails\": {emails}, \"children\": {children}}}")
            }
        };
        lines.push(format!(
            "{{\"id\": {id}, \"name\": {name}, \"address\": {address}, \"tags\": {tags}, \
             \"scores\": {scores}, \"matrix\": {matrix}, \"orders\": {orders}, \
             \"attrs\": {attrs}, \"history\": {history}, \"profile\": {profile}}}"
        ));
    }
    // Arrow's JSON reader takes only text map keys: read `history` with
    // text keys, then cast them to whole numbers.
    let text_keys = deep_nested_schema_with(false);
    let schema = deep_nested_schema();
    let reader = arrow::json::ReaderBuilder::new(text_keys)
        .with_batch_size(100)
        .build(std::io::Cursor::new(lines.join("\n")))
        .unwrap();
    let batches: Vec<RecordBatch> = reader
        .map(|batch| {
            let batch = batch.unwrap();
            let columns: Vec<ArrayRef> = batch
                .columns()
                .iter()
                .zip(schema.fields())
                .map(|(c, f)| arrow::compute::cast(c, f.data_type()).unwrap())
                .collect();
            RecordBatch::try_new(schema.clone(), columns).unwrap()
        })
        .collect();
    write(path, &batches, WriterProperties::builder().build());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TempDir;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::file::reader::{FileReader, SerializedFileReader};

    #[test]
    fn all_types_round_trips() {
        let dir = TempDir::new();
        let path = dir.join("all_types.parquet");
        all_types(&path);

        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.map(Result::unwrap).collect();
        let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
        assert_eq!(rows, ALL_TYPES_ROWS);
        let batch = &batches[0];
        assert_eq!(batch.num_columns(), 19);
        for column in batch.columns() {
            assert_eq!(column.null_count(), 1);
            assert!(column.is_null(2));
        }
    }

    #[test]
    fn mixed_settings_has_expected_layout() {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        mixed_settings(&path);

        let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
        let meta = reader.metadata();
        assert_eq!(
            meta.file_metadata().num_rows() as usize,
            MIXED_SETTINGS_ROWS
        );
        assert_eq!(
            meta.num_row_groups(),
            MIXED_SETTINGS_ROWS.div_ceil(MIXED_SETTINGS_ROW_GROUP)
        );

        let kv = meta.file_metadata().key_value_metadata().unwrap();
        for (key, value) in MIXED_SETTINGS_METADATA {
            let entry = kv.iter().find(|e| e.key == *key).unwrap();
            assert_eq!(entry.value.as_deref(), Some(*value));
        }

        let rg = meta.row_group(0);
        let id = rg.column(0);
        assert!(matches!(id.compression(), Compression::ZSTD(_)));
        assert!(id.encodings().any(|e| e == Encoding::DELTA_BINARY_PACKED));
        assert!(id.dictionary_page_offset().is_none());

        let name = rg.column(1);
        assert_eq!(name.compression(), Compression::SNAPPY);
        assert!(name.dictionary_page_offset().is_some());

        let score = rg.column(2);
        assert!(matches!(score.compression(), Compression::GZIP(_)));
        assert!(score.encodings().any(|e| e == Encoding::BYTE_STREAM_SPLIT));
        assert!(score.statistics().is_none());

        assert_eq!(rg.column(3).compression(), Compression::UNCOMPRESSED);
    }

    #[test]
    fn large_has_requested_rows_and_row_groups() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        large(&path, 100_000, 30_000);

        let reader = SerializedFileReader::new(File::open(&path).unwrap()).unwrap();
        let meta = reader.metadata();
        assert_eq!(meta.file_metadata().num_rows(), 100_000);
        assert_eq!(meta.num_row_groups(), 4);
    }

    #[test]
    fn temp_dir_is_removed_on_drop() {
        let dir = TempDir::new();
        let path = dir.path().to_owned();
        assert!(path.is_dir());
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn deep_nested_round_trips() {
        let dir = TempDir::new();
        let path = dir.join("deep.parquet");
        deep_nested(&path, 500);
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.map(Result::unwrap).collect();
        assert_eq!(
            batches.iter().map(RecordBatch::num_rows).sum::<usize>(),
            500
        );
        let schema = batches[0].schema();
        for (field, expected) in schema.fields().iter().zip(deep_nested_schema().fields()) {
            assert_eq!(field.name(), expected.name());
        }
        assert!(matches!(
            schema.field_with_name("history").unwrap().data_type(),
            DataType::Map(..)
        ));
        // Every nested column has some nulls, but mostly values.
        for i in 2..schema.fields().len() {
            let nulls: usize = batches.iter().map(|b| b.column(i).null_count()).sum();
            assert!(nulls > 0 && nulls < 100, "{}", schema.field(i).name());
        }
    }
}
