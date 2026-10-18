//! CSV and JSON: import, export and round trips through Parquet.

#![allow(clippy::unwrap_used, clippy::single_range_in_vec_init)] // Tests.

use veta_core::convert::{
    CsvExportOptions, CsvOptions, ExportFormat, export, import_csv, import_json,
};
use veta_core::display::{format_batch, type_name};
use veta_core::{Document, OpenOptions};
use veta_testkit::{TempDir, fixtures};

const CSV: &str = "id,name,score,flag\n1,Ann,1.5,true\n2,\"Smith, Bo\",,false\n3,Cé,-2.25,\n";

fn all(doc: &Document) -> Vec<Vec<Option<String>>> {
    format_batch(&doc.read(0..doc.num_rows()).unwrap()).unwrap()
}

fn types(doc: &Document) -> Vec<String> {
    doc.schema()
        .fields()
        .iter()
        .map(|f| type_name(f.data_type()))
        .collect()
}

fn go(_: f32) -> bool {
    true
}

#[test]
fn csv_import_guesses_types_and_round_trips() {
    let dir = TempDir::new();
    let path = dir.join("in.csv");
    std::fs::write(&path, CSV).unwrap();
    let (doc, report) = import_csv(
        &path,
        &CsvOptions::default(),
        veta_core::memory::budget(),
        &mut go,
    )
    .unwrap();
    assert_eq!(report.rows, 3);
    assert!(report.failures.is_empty());
    assert_eq!(types(&doc), ["int64", "string", "float64", "bool"]);
    assert_eq!(doc.num_rows(), 3);
    assert_eq!(all(&doc)[1][1].as_deref(), Some("Smith, Bo"));
    assert_eq!(all(&doc)[1][2], None);

    let out = dir.join("out.csv");
    let format = ExportFormat::Csv(CsvExportOptions::default());
    export(&doc.reader(), &out, &format, &mut go).unwrap();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), CSV);
}

#[test]
fn csv_with_semicolons_and_decimal_commas() {
    let dir = TempDir::new();
    let path = dir.join("eu.csv");
    std::fs::write(&path, "a;b\n1,5;x\n-2,25;y\n").unwrap();
    let options = CsvOptions {
        delimiter: b';',
        decimal: ',',
        ..CsvOptions::default()
    };
    let (doc, _) = import_csv(&path, &options, veta_core::memory::budget(), &mut go).unwrap();
    assert_eq!(types(&doc), ["float64", "string"]);
    assert_eq!(all(&doc)[1][0].as_deref(), Some("-2.25"));
    let out = dir.join("eu-out.csv");
    let format = ExportFormat::Csv(CsvExportOptions {
        delimiter: b';',
        decimal: ',',
        ..CsvExportOptions::default()
    });
    export(&doc.reader(), &out, &format, &mut go).unwrap();
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "a;b\n1,5;x\n-2,25;y\n"
    );
}

#[test]
fn csv_values_that_do_not_fit_are_reported() {
    let dir = TempDir::new();
    let path = dir.join("bad.csv");
    std::fs::write(&path, "n\n1\n2\nthree\n").unwrap();
    let options = CsvOptions {
        types: Some(vec![veta_core::arrow::datatypes::DataType::Int64]),
        ..CsvOptions::default()
    };
    let (doc, report) = import_csv(&path, &options, veta_core::memory::budget(), &mut go).unwrap();
    assert_eq!(report.failures, [("n".to_owned(), 1)]);
    assert!(report.summary().is_some());
    // The value is kept in an error column next to its column.
    assert_eq!(doc.schema().fields().len(), 2);
    assert_eq!(all(&doc)[2][0], None);
}

#[test]
fn json_round_trips_through_parquet() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    let doc = Document::open(&path, OpenOptions::default()).unwrap();
    for (name, format) in [
        ("rows.jsonl", ExportFormat::JsonLines),
        ("rows.json", ExportFormat::JsonArray),
    ] {
        let out = dir.join(name);
        export(&doc.reader(), &out, &format, &mut go).unwrap();
        let mut imported = import_json(&out, veta_core::memory::budget(), &mut go).unwrap();
        assert_eq!(imported.num_rows(), doc.num_rows());
        assert_eq!(all(&imported), all(&doc), "{name}");

        // And on to Parquet and back.
        let parquet = dir.join(format!("{name}.parquet").as_str());
        imported.save(Some(parquet.clone())).unwrap();
        let saved = Document::open(&parquet, OpenOptions::default()).unwrap();
        assert_eq!(all(&saved), all(&doc), "{name} saved");
    }
}

#[test]
fn nested_values_export_as_json() {
    let dir = TempDir::new();
    let path = dir.join("nested.parquet");
    fixtures::nested(&path);
    let doc = Document::open(&path, OpenOptions::default()).unwrap();
    let out = dir.join("nested.jsonl");
    export(&doc.reader(), &out, &ExportFormat::JsonLines, &mut go).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["point"]["x"], 1);
    assert_eq!(first["tags"][1], "b");
    let imported = import_json(&out, veta_core::memory::budget(), &mut go).unwrap();
    assert_eq!(imported.num_rows(), doc.num_rows());
}
