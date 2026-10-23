//! Tests for `transforms::filter`.

use super::*;
use arrow::array::{Float64Array, Int64Array};
use arrow::datatypes::Field;

fn batch() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Float64, true),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
            Arc::new(StringArray::from(vec![
                Some("Ana"),
                Some("bob"),
                None,
                Some("anabel"),
            ])),
            Arc::new(Float64Array::from(vec![
                Some(1.5),
                None,
                Some(3.0),
                Some(10.0),
            ])),
        ],
    )
    .unwrap()
}

fn kept(filter: &Filter) -> Vec<bool> {
    filter
        .evaluate(&batch())
        .unwrap()
        .iter()
        .map(|b| b.unwrap())
        .collect()
}

fn condition(column: &str, op: FilterOp, value: &str) -> Condition {
    Condition {
        column: column.into(),
        op,
        value: value.into(),
        case_sensitive: false,
    }
}

fn one(c: Condition) -> Filter {
    Filter {
        conditions: vec![c],
        match_all: true,
        exclude: false,
    }
}

#[test]
fn comparisons_skip_nulls() {
    assert_eq!(
        kept(&one(condition("score", FilterOp::Greater, "2"))),
        [false, false, true, true]
    );
    assert_eq!(
        kept(&one(condition("id", FilterOp::NotEquals, "2"))),
        [true, false, true, true]
    );
    assert_eq!(
        kept(&one(condition("score", FilterOp::IsNull, ""))),
        [false, true, false, false]
    );
}

#[test]
fn text_tests_ignore_case_unless_asked() {
    assert_eq!(
        kept(&one(condition("name", FilterOp::StartsWith, "ana"))),
        [true, false, false, true]
    );
    let mut sensitive = condition("name", FilterOp::Contains, "ana");
    sensitive.case_sensitive = true;
    assert_eq!(kept(&one(sensitive)), [false, false, false, true]);
    assert_eq!(
        kept(&one(condition("name", FilterOp::Equals, "BOB"))),
        [false, true, false, false]
    );
    // Text tests work on other types through their text.
    assert_eq!(
        kept(&one(condition("score", FilterOp::Contains, ".5"))),
        [true, false, false, false]
    );
}

#[test]
fn any_and_all() {
    let mut filter = Filter {
        conditions: vec![
            condition("id", FilterOp::Less, "2"),
            condition("name", FilterOp::EndsWith, "bel"),
        ],
        match_all: false,
        exclude: false,
    };
    assert_eq!(kept(&filter), [true, false, false, true]);
    filter.match_all = true;
    assert_eq!(kept(&filter), [false, false, false, false]);
    // Excluding removes the matching rows, rows with nulls included.
    filter.match_all = false;
    filter.exclude = true;
    assert_eq!(kept(&filter), [false, true, true, false]);
}

#[test]
fn validation() {
    let schema = batch().schema();
    assert!(
        one(condition("score", FilterOp::Greater, "abc"))
            .validate(&schema)
            .is_err()
    );
    assert!(
        one(condition("nope", FilterOp::IsNull, ""))
            .validate(&schema)
            .is_err()
    );
    assert!(
        one(condition("score", FilterOp::Greater, ""))
            .validate(&schema)
            .is_err()
    );
    assert!(
        one(condition("name", FilterOp::Equals, ""))
            .validate(&schema)
            .is_ok()
    );
    assert!(
        Filter {
            conditions: vec![],
            match_all: true,
            exclude: false
        }
        .validate(&schema)
        .is_err()
    );
}

#[test]
fn nested_columns_filter_as_json() {
    let dir = veta_testkit::TempDir::new();
    let path = dir.join("n.parquet");
    veta_testkit::fixtures::nested(&path);
    let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
    let batch = doc.read(0..4).unwrap();
    let kept = |column: &str, op: FilterOp, value: &str| -> Vec<usize> {
        let filter = Filter {
            conditions: vec![Condition {
                column: column.into(),
                op,
                value: value.into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        };
        filter.validate(&batch.schema()).unwrap();
        let mask = filter.evaluate(&batch).unwrap();
        (0..mask.len()).filter(|&i| mask.value(i)).collect()
    };
    assert_eq!(kept("tags", FilterOp::Contains, "\"A\""), [0]);
    assert_eq!(kept("tags", FilterOp::Equals, r#"[ "c", null, "d" ]"#), [3]);
    assert_eq!(kept("point", FilterOp::Equals, r#"{"y": 2, "x": 1}"#), [0]);
    assert_eq!(
        kept("point", FilterOp::NotEquals, r#"{"x": 1, "y": 2}"#),
        [1, 3]
    );
    assert_eq!(kept("scores", FilterOp::Contains, "math"), [0, 3]);
    assert_eq!(kept("owner", FilterOp::IsNull, ""), [2]);
    let less = Filter {
        conditions: vec![Condition {
            column: "tags".into(),
            op: FilterOp::Less,
            value: "[]".into(),
            case_sensitive: false,
        }],
        match_all: true,
        exclude: false,
    };
    assert!(less.validate(&batch.schema()).is_err());
}

#[test]
fn ignoring_case_matches_text_literally() {
    let schema = Arc::new(Schema::new(vec![Field::new("t", DataType::Utf8, true)]));
    let batch = RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from(vec![
            Some("50% OFF"),
            Some("500 off"),
            Some("A_b"),
            Some("axb"),
            Some("ÑANDÚ"),
            Some("back\\slash"),
            None,
        ]))],
    )
    .unwrap();
    let test = |op, value: &str| -> Vec<bool> {
        let filter = Filter {
            conditions: vec![Condition {
                column: "t".into(),
                op,
                value: value.into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        };
        let mask = filter.evaluate(&batch).unwrap();
        (0..mask.len())
            .map(|i| mask.is_valid(i) && mask.value(i))
            .collect()
    };
    // % and _ are text here, not wildcards.
    assert_eq!(
        test(FilterOp::Contains, "0% off"),
        [true, false, false, false, false, false, false]
    );
    assert_eq!(
        test(FilterOp::Equals, "a_B"),
        [false, false, true, false, false, false, false]
    );
    assert_eq!(
        test(FilterOp::StartsWith, "ñan"),
        [false, false, false, false, true, false, false]
    );
    assert_eq!(
        test(FilterOp::EndsWith, "\\SLASH"),
        [false, false, false, false, false, true, false]
    );
    // Not equal / not containing: nulls are never kept.
    assert_eq!(
        test(FilterOp::NotContains, "OFF"),
        [false, false, true, true, true, true, false]
    );
}
