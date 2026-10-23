//! Tests for `transforms::cast`.

use super::*;
use arrow::array::{Float64Array, Int64Array};

fn text(values: &[Option<&str>]) -> ArrayRef {
    Arc::new(StringArray::from(values.to_vec()))
}

fn cast(to: DataType, options: CastOptions) -> Cast {
    Cast {
        column: "c".into(),
        to,
        options,
    }
}

#[test]
fn numbers_with_locale_separators() {
    let c = cast(
        DataType::Float64,
        CastOptions {
            decimal: ',',
            thousands: Some('.'),
            ..CastOptions::default()
        },
    );
    let out = c
        .convert(&text(&[Some("1.234,5"), Some(" 7 "), Some("x"), None]))
        .unwrap();
    let out = out.as_any().downcast_ref::<Float64Array>().unwrap();
    assert_eq!(
        out.iter().collect::<Vec<_>>(),
        [Some(1234.5), Some(7.0), None, None]
    );

    let preview = c.preview(&text(&[Some("1,5"), Some("x"), Some("y"), Some("x"), None]));
    assert_eq!(
        preview,
        CastPreview {
            checked: 5,
            failed: 3,
            examples: vec!["x".into(), "y".into()]
        }
    );
}

#[test]
fn dates_with_formats() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(days_from_civil(1969, 12, 31), -1);
    let parse = |s, f| parse_datetime(s, f);
    assert_eq!(parse("31/12/2024", "%d/%m/%Y").unwrap().day, 31);
    assert!(parse("31/02/2024", "%d/%m/%Y").is_none());
    assert!(parse("29/02/2024", "%d/%m/%Y").is_some());
    assert!(parse("12/31/2024x", "%m/%d/%Y").is_none());
    let dt = parse("5-Mar-24 07:08:09.25 PM", "%d-%b-%y %I:%M:%S.%f %p").unwrap();
    assert_eq!(
        (dt.year, dt.month, dt.day, dt.hour, dt.nanos),
        (2024, 3, 5, 19, 250_000_000)
    );

    let c = cast(
        DataType::Timestamp(TimeUnit::Microsecond, None),
        CastOptions {
            format: Some("%d.%m.%Y %H:%M".into()),
            ..CastOptions::default()
        },
    );
    let out = c
        .convert(&text(&[Some("02.01.1970 00:01"), Some("bad")]))
        .unwrap();
    let out = out
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .unwrap();
    assert_eq!(
        out.iter().collect::<Vec<_>>(),
        [Some((86_400 + 60) * 1_000_000), None]
    );
    assert!(check_format("%d/%q").is_err());
    assert!(check_format("dd/mm").is_err());
}

#[test]
fn error_modes() {
    let schema = Arc::new(Schema::new(vec![
        Field::new("c", DataType::Utf8, true),
        Field::new("d", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            text(&[Some("1"), Some("two"), None]),
            Arc::new(Int64Array::from(vec![1, 2, 3])),
        ],
    )
    .unwrap();
    let run = |on_error| {
        let c = cast(
            DataType::Int64,
            CastOptions {
                on_error,
                ..CastOptions::default()
            },
        );
        let out = Arc::new(Schema::new(c.output_fields(&schema).unwrap()));
        c.apply(&batch, &out)
    };
    let nulls = run(OnError::Null).unwrap();
    assert_eq!(nulls.column(0).null_count(), 2);
    assert!(
        run(OnError::Fail)
            .unwrap_err()
            .to_string()
            .contains("\"two\"")
    );
    let errors = run(OnError::ErrorColumn).unwrap();
    assert_eq!(errors.schema().field(1).name(), "c errors");
    let column = errors
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(column.iter().collect::<Vec<_>>(), [None, Some("two"), None]);

    // Non-text columns use arrow's conversions.
    let to_text = cast(DataType::Utf8, CastOptions::default());
    let fields = to_text.output_fields(&schema);
    assert!(fields.is_ok());
    let bad = Cast {
        column: "d".into(),
        to: DataType::Date32,
        options: CastOptions {
            format: Some("%Y".into()),
            ..CastOptions::default()
        },
    };
    assert!(bad.output_fields(&schema).is_err());
    let wrong_target = cast(
        DataType::Int64,
        CastOptions {
            format: Some("%Y".into()),
            ..CastOptions::default()
        },
    );
    assert!(wrong_target.output_fields(&schema).is_err());
}

#[test]
fn numbers_to_text_with_separators() {
    let numbers: ArrayRef = Arc::new(arrow::array::Float64Array::from(vec![
        Some(-1234567.5),
        Some(12.0),
        None,
    ]));
    let c = cast(
        DataType::Utf8,
        CastOptions {
            decimal: ',',
            thousands: Some('.'),
            ..CastOptions::default()
        },
    );
    let out = c.convert(&numbers).unwrap();
    let out = out.as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(
        out.iter().collect::<Vec<_>>(),
        [Some("-1.234.567,5"), Some("12,0"), None]
    );
}

#[test]
fn month_names_in_several_languages() {
    let dates = text(&[
        Some("3 marzo 2024"),
        Some("3 mars 2024"),
        Some("3 Okt. 2024"),
        Some("3 dic 2024"),
        Some("3 juin 2024"),
        Some("3 jui 2024"), // juin or juillet: ambiguous
    ]);
    let c = cast(
        DataType::Date32,
        CastOptions {
            format: Some("%d %b %Y".into()),
            ..CastOptions::default()
        },
    );
    let out = c.convert(&dates).unwrap();
    let out = arrow::compute::cast(&out, &DataType::Utf8).unwrap();
    let out = out.as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(
        out.iter().collect::<Vec<_>>(),
        [
            Some("2024-03-03"),
            Some("2024-03-03"),
            Some("2024-10-03"),
            Some("2024-12-03"),
            Some("2024-06-03"),
            None
        ]
    );
}

#[test]
fn text_to_nested_and_more_type_names() {
    use crate::display::parse_type_name;
    let to = parse_type_name("list<int64>").unwrap();
    let c = cast(to.clone(), CastOptions::default());
    let input = text(&[Some("[1, 2]"), Some("nope"), None]);
    let schema = Schema::new(vec![Field::new("c", DataType::Utf8, true)]);
    assert!(c.output_fields(&schema).is_ok());
    let out = c.convert(&input).unwrap();
    assert_eq!(out.data_type(), &to);
    assert_eq!(out.null_count(), 2);
    assert_eq!(
        parse_type_name("duration[ms]"),
        Some(DataType::Duration(TimeUnit::Millisecond))
    );
    assert_eq!(
        parse_type_name("decimal128(10, 2)"),
        Some(DataType::Decimal128(10, 2))
    );
    assert_eq!(
        parse_type_name("timestamp[ms, Europe/Madrid]"),
        Some(DataType::Timestamp(
            TimeUnit::Millisecond,
            Some("Europe/Madrid".into())
        ))
    );
}
