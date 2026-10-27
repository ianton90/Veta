//! Statistics through the `DataFrame` API: column methods, several in one
//! pass, describe.

#![allow(clippy::unwrap_used)] // Tests.

use std::sync::Arc;

use arrow::array::{Array, AsArray, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Int64Type, Schema};
use arrow::record_batch::RecordBatch;
use veta_core::{DataFrame, Interpolation, Stat, StatValue};

/// x: 1..=n with a null every 10th row; name: "a", "b", "b", "c" repeated.
fn frame(n: i64) -> DataFrame {
    let schema = Arc::new(Schema::new(vec![
        Field::new("x", DataType::Int64, true),
        Field::new("name", DataType::Utf8, false),
        Field::new("f", DataType::Float64, true),
    ]));
    let xs: Int64Array = (1..=n).map(|i| (i % 10 != 0).then_some(i)).collect();
    let names: StringArray = (0..n)
        .map(|i| Some(["a", "b", "b", "c"][(i % 4) as usize]))
        .collect();
    let fs: Float64Array = (1..=n).map(|i| Some(i as f64 * 0.5)).collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(xs), Arc::new(names), Arc::new(fs)],
    )
    .unwrap();
    DataFrame::from_batches(schema, vec![batch]).unwrap()
}

/// The values of x, as pandas would see them.
fn xs(n: i64) -> Vec<f64> {
    (1..=n).filter(|i| i % 10 != 0).map(|i| i as f64).collect()
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() <= 1e-9 * b.abs().max(1.0))
}

#[test]
fn column_methods() {
    let n = 200_000;
    let values = xs(n);
    let count = values.len() as f64;
    let mean = values.iter().sum::<f64>() / count;
    let var = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count;
    for df in [frame(n), frame(n).lazy()] {
        let x = df.column("x").unwrap();
        assert_eq!(x.len().unwrap(), n as usize);
        assert_eq!(x.count_values().unwrap(), values.len() as u64);
        assert_eq!(x.count_nulls().unwrap(), (n / 10) as u64);
        assert_eq!(x.n_unique().unwrap(), values.len() as u64);
        assert_eq!(x.min().unwrap().as_primitive::<Int64Type>().value(0), 1);
        assert_eq!(x.max().unwrap().as_primitive::<Int64Type>().value(0), n - 1);
        assert!(close(Some(x.sum().unwrap()), values.iter().sum()));
        assert!(close(x.mean().unwrap(), mean));
        assert!(close(x.var(false).unwrap(), var));
        assert!(close(x.std(false).unwrap(), var.sqrt()));
        assert!(close(x.var(true).unwrap(), var * count / (count - 1.0)));
        assert!(close(
            x.std(true).unwrap(),
            (var * count / (count - 1.0)).sqrt()
        ));
        let mut sorted = values.clone();
        sorted.sort_by(f64::total_cmp);
        let at = 0.25 * (count - 1.0);
        let (low, high) = (sorted[at.floor() as usize], sorted[at.ceil() as usize]);
        assert!(close(
            x.quantile(0.25, Interpolation::Linear).unwrap(),
            low + (high - low) * (at - at.floor())
        ));
        assert!(close(x.quantile(0.25, Interpolation::Lower).unwrap(), low));
        assert!(close(
            x.median().unwrap(),
            sorted[sorted.len() / 2 - 1] / 2.0 + sorted[sorted.len() / 2] / 2.0
        ));
        // Evenly spread: no skew; flat: excess kurtosis -1.2 either way, for
        // this many values.
        assert!(x.skew(false).unwrap().unwrap().abs() < 1e-3);
        assert!(x.skew(true).unwrap().unwrap().abs() < 1e-3);
        assert!((x.kurtosis(false).unwrap().unwrap() + 1.2).abs() < 1e-4);
        assert!((x.kurtosis(true).unwrap().unwrap() + 1.2).abs() < 1e-4);
        let name = df.column("name").unwrap();
        assert_eq!(name.n_unique().unwrap(), 3);
        assert_eq!(name.mode().unwrap().as_string::<i32>().value(0), "b");
        assert_eq!(name.min().unwrap().as_string::<i32>().value(0), "a");
        assert!(name.mean().is_err(), "text has no mean");
        assert!(name.sum().is_err());
    }
}

#[test]
fn several_in_one_pass_and_describe() {
    let df = frame(1000);
    let stats = df
        .stats(
            ["x", "f"],
            &[
                Stat::CountValues,
                Stat::Mean,
                Stat::Max,
                Stat::Quantile(0.5),
            ],
        )
        .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0][0].as_count(), Some(900));
    assert!(close(stats[1][1].as_number(), 250.25));
    assert!(
        matches!(&stats[1][2], StatValue::Value(v) if v.as_primitive::<arrow::datatypes::Float64Type>().value(0) == 500.0)
    );
    let summary = df.describe().unwrap();
    assert_eq!(summary.columns(), ["statistic", "x", "name", "f"]);
    let column = |name: &str| -> Vec<Option<String>> {
        let values = summary.column(name).unwrap().values().unwrap();
        values
            .as_string::<i32>()
            .iter()
            .map(|s| s.map(str::to_owned))
            .collect()
    };
    assert_eq!(column("statistic")[0].as_deref(), Some("count"));
    assert_eq!(column("x")[0].as_deref(), Some("900"));
    assert_eq!(column("x")[1].as_deref(), Some("100"));
    assert_eq!(column("name")[2].as_deref(), Some("3"));
    assert_eq!(column("name")[3], None, "text has no mean");
    assert_eq!(column("f")[9].as_deref(), Some("500.0"));
    assert_eq!(column("f")[3].as_deref(), Some("250.25"));
}

#[test]
fn on_a_sample_and_empty() {
    let df = frame(100_000);
    let sampled = df.sample(0.1, 1).unwrap();
    let mean = sampled.column("x").unwrap().mean().unwrap().unwrap();
    assert!((mean - 50_000.0).abs() < 1_000.0, "{mean}");
    let empty = df.keep_rows(0..0).unwrap();
    let x = empty.column("x").unwrap();
    assert_eq!(x.mean().unwrap(), None);
    assert_eq!(x.std(false).unwrap(), None);
    assert!(x.min().unwrap().is_null(0));
    assert_eq!(x.n_unique().unwrap(), 0);
}
