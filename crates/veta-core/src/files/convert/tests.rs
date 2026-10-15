use super::*;
use crate::display::format_batch;
use veta_testkit::TempDir;

fn cells(doc: &Document) -> Vec<Vec<Option<String>>> {
    format_batch(&doc.read(0..doc.num_rows()).unwrap()).unwrap()
}

fn types(doc: &Document) -> Vec<DataType> {
    doc.schema()
        .fields()
        .iter()
        .map(|f| f.data_type().clone())
        .collect()
}

#[test]
fn csv_import_guesses_types() {
    let dir = TempDir::new();
    let path = dir.join("people.csv");
    std::fs::write(
        &path,
        "\u{feff}id,name,score,ok,when\n1,Ana,1.5,true,2024-01-02\n2,\"Bo, Jr.\",,false,2024-02-03\n3,Cy,2,true,\n",
    )
    .unwrap();
    let doc = import_csv(&path, &CsvOptions::default(), 1 << 30, &mut |_| true)
        .unwrap()
        .0;
    assert_eq!(doc.title(), "people.csv");
    assert!(doc.is_modified() && doc.path().is_none());
    assert_eq!(
        types(&doc),
        [
            DataType::Int64,
            DataType::Utf8,
            DataType::Float64,
            DataType::Boolean,
            DataType::Date32
        ]
    );
    let c = cells(&doc);
    assert_eq!(c[1][1].as_deref(), Some("Bo, Jr."));
    assert_eq!(c[1][2], None);
    assert_eq!(c[2][4], None);
}

#[test]
fn csv_options_and_type_overrides() {
    let dir = TempDir::new();
    let path = dir.join("eu.csv");
    // Latin-1 "é" and decimal commas, semicolon separated, no header.
    let mut bytes = b"1;Caf".to_vec();
    bytes.push(0xE9);
    bytes.extend_from_slice(b";1,5\n2;X;abc\n");
    std::fs::write(&path, bytes).unwrap();
    let mut options = CsvOptions {
        delimiter: b';',
        header: false,
        encoding: TextEncoding::Latin1,
        decimal: ',',
        ..CsvOptions::default()
    };
    let preview = preview_csv(&path, &options, 10).unwrap();
    // "abc" makes the third column text.
    assert_eq!(preview.schema.field(2).data_type(), &DataType::Utf8);
    options.types = Some(vec![DataType::Utf8, DataType::Utf8, DataType::Float64]);
    let doc = import_csv(&path, &options, 1 << 30, &mut |_| true)
        .unwrap()
        .0;
    let c = cells(&doc);
    assert_eq!(c[0][1].as_deref(), Some("Café"));
    assert_eq!(c[0][2].as_deref(), Some("1.5"));
    // The invalid value is kept as text in an error column.
    assert_eq!(c[1][2], None);
    assert_eq!(doc.schema().field(3).name(), "column_3 errors");
    assert_eq!(c[1][3].as_deref(), Some("abc"));
    options.types = Some(vec![DataType::Utf8]);
    assert!(import_csv(&path, &options, 1 << 30, &mut |_| true).is_err());
}

#[test]
fn invalid_values_are_reported() {
    let dir = TempDir::new();
    let path = dir.join("bad.csv");
    std::fs::write(&path, "n,m,s\n1,2,a\nx,3,b\n4,y,c\n5,6,d\n").unwrap();
    let types = Some(vec![DataType::Int64, DataType::Int64, DataType::Utf8]);
    let run = |on_error| {
        let options = CsvOptions {
            types: types.clone(),
            on_error,
            ..CsvOptions::default()
        };
        import_csv(&path, &options, 1 << 30, &mut |_| true)
    };
    let (doc, report) = run(OnError::ErrorColumn).unwrap();
    let names: Vec<_> = doc
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["n", "n errors", "m", "m errors", "s"]);
    assert_eq!(cells(&doc)[1][1].as_deref(), Some("x"));
    assert_eq!(report.failures, [("n".to_owned(), 1), ("m".to_owned(), 1)]);
    assert!(report.summary().unwrap().contains("errors"));

    let (doc, report) = run(OnError::Null).unwrap();
    assert_eq!(doc.num_columns(), 3);
    assert!(report.summary().unwrap().contains("null"));

    let err = run(OnError::Fail).unwrap_err().to_string();
    assert!(err.contains("ow 2") && err.contains("\"x\""), "{err}");

    // Nothing failed: no error columns.
    let fine = dir.join("fine.csv");
    std::fs::write(&fine, "n\n1\n2\n").unwrap();
    let (doc, report) = import_csv(&fine, &CsvOptions::default(), 1 << 30, &mut |_| true).unwrap();
    assert_eq!(doc.num_columns(), 1);
    assert_eq!(report.summary(), None);

    let preview = preview_csv(
        &path,
        &CsvOptions {
            types,
            ..CsvOptions::default()
        },
        10,
    )
    .unwrap();
    assert_eq!(preview.failures, [1, 1, 0]);
}

#[test]
fn quote_characters() {
    let dir = TempDir::new();
    let path = dir.join("q.csv");
    std::fs::write(&path, "a|b\n'x|y'|\"z\"\n").unwrap();
    let read = |quote| {
        let options = CsvOptions {
            delimiter: b'|',
            quote,
            ..CsvOptions::default()
        };
        cells(
            &import_csv(&path, &options, 1 << 30, &mut |_| true)
                .unwrap()
                .0,
        )
    };
    assert_eq!(
        read(Some(b'\'')),
        [[Some("x|y".to_owned()), Some("\"z\"".into())]]
    );
    // Without quoting, quotes are ordinary text.
    std::fs::write(&path, "a|b\n'x'|\"z\"\n").unwrap();
    assert_eq!(read(None), [[Some("'x'".to_owned()), Some("\"z\"".into())]]);
    std::fs::write(&path, "a|b\n'x|y'|\"z\"\n").unwrap();

    let out = dir.join("out.csv");
    let doc = import_csv(
        &path,
        &CsvOptions {
            delimiter: b'|',
            quote: Some(b'\''),
            ..CsvOptions::default()
        },
        1 << 30,
        &mut |_| true,
    )
    .unwrap()
    .0;
    let export_with = |quote| {
        let options = CsvExportOptions {
            delimiter: b'|',
            quote,
            ..CsvExportOptions::default()
        };
        export(
            &doc.output().unwrap(),
            &out,
            &ExportFormat::Csv(options),
            &mut |_| true,
        )
        .unwrap();
        std::fs::read_to_string(&out).unwrap()
    };
    assert_eq!(export_with(Some(b'\'')).lines().nth(1), Some("'x|y'|\"z\""));
    assert_eq!(export_with(None).lines().nth(1), Some("x|y|\"z\""));
}

#[test]
fn large_csv_is_paged_and_cancellable() {
    let dir = TempDir::new();
    let path = dir.join("big.csv");
    let mut text = String::from("n,label\n");
    for i in 0..200_000 {
        text.push_str(&format!("{i},row {i}\n"));
    }
    std::fs::write(&path, text).unwrap();
    let doc = import_csv(&path, &CsvOptions::default(), 1024, &mut |_| true)
        .unwrap()
        .0;
    assert_eq!(doc.source_mode(), crate::SourceMode::Paged);
    assert_eq!(doc.num_rows(), 200_000);
    let last = format_batch(&doc.read(199_999..200_000).unwrap()).unwrap();
    assert_eq!(last[0][1].as_deref(), Some("row 199999"));
    let result = import_csv(&path, &CsvOptions::default(), 1024, &mut |f| f < 0.5);
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn json_lines_and_arrays() {
    let dir = TempDir::new();
    let lines = dir.join("a.ndjson");
    std::fs::write(
        &lines,
        "{\"a\": 1, \"b\": \"x\"}\n{\"a\": 2, \"b\": null, \"c\": {\"d\": [1, 2]}}\n",
    )
    .unwrap();
    let array = dir.join("a.json");
    std::fs::write(
        &array,
        "[\n {\"a\": 1, \"b\": \"x\"},\n {\"a\": 2, \"b\": null, \"c\": {\"d\": [1, 2]}}\n]",
    )
    .unwrap();
    let from_lines = import_json(&lines, 1 << 30, &mut |_| true).unwrap();
    let from_array = import_json(&array, 1 << 30, &mut |_| true).unwrap();
    assert_eq!(from_lines.num_rows(), 2);
    assert_eq!(cells(&from_lines), cells(&from_array));
    assert!(matches!(
        from_lines.schema().field(2).data_type(),
        DataType::Struct(_)
    ));
    assert_eq!(preview_json(&array, 1).unwrap().num_rows(), 1);
    let tricky = dir.join("t.json");
    std::fs::write(&tricky, r#"[{"s": "a]},{\"b"}]"#).unwrap();
    let doc = import_json(&tricky, 1 << 30, &mut |_| true).unwrap();
    assert_eq!(cells(&doc)[0][0].as_deref(), Some("a]},{\"b"));
    let bad = dir.join("bad.json");
    std::fs::write(&bad, "[1, 2]").unwrap();
    assert!(import_json(&bad, 1 << 30, &mut |_| true).is_err());
}

#[test]
fn export_round_trips() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    veta_testkit::fixtures::mixed_settings(&path);
    let doc = Document::open(&path, crate::OpenOptions::default()).unwrap();
    let output = doc.output().unwrap();

    let csv = dir.join("out.csv");
    export(
        &output,
        &csv,
        &ExportFormat::Csv(CsvExportOptions::default()),
        &mut |_| true,
    )
    .unwrap();
    let back = import_csv(&csv, &CsvOptions::default(), 1 << 30, &mut |_| true)
        .unwrap()
        .0;
    assert_eq!(cells(&back), cells(&doc));

    let eu = dir.join("eu.csv");
    let options = CsvExportOptions {
        delimiter: b';',
        decimal: ',',
        ..CsvExportOptions::default()
    };
    export(&output, &eu, &ExportFormat::Csv(options), &mut |_| true).unwrap();
    let text = std::fs::read_to_string(&eu).unwrap();
    assert!(text.lines().nth(2).unwrap().contains(";0,5;"), "{text}");
    let back = import_csv(
        &eu,
        &CsvOptions {
            delimiter: b';',
            decimal: ',',
            ..CsvOptions::default()
        },
        1 << 30,
        &mut |_| true,
    )
    .unwrap()
    .0;
    assert_eq!(cells(&back), cells(&doc));

    for format in [ExportFormat::JsonLines, ExportFormat::JsonArray] {
        let json = dir.join("out.json");
        export(&output, &json, &format, &mut |_| true).unwrap();
        let back = import_json(&json, 1 << 30, &mut |_| true).unwrap();
        assert_eq!(back.num_rows(), 1000);
        assert_eq!(cells(&back)[1], cells(&doc)[1]);
    }
    assert!(matches!(
        export(
            &output,
            &dir.join("x.csv"),
            &ExportFormat::JsonLines,
            &mut |_| false
        ),
        Err(Error::Cancelled)
    ));
    assert!(!dir.join("x.csv").exists());
}

#[test]
fn more_encodings_dates_and_late_json_keys() {
    let dir = TempDir::new();
    // UTF-16 LE with a byte order mark, read with the default (UTF-8).
    let path = dir.join("u16.csv");
    let text = "name,when\nzoë,31/12/2024\n€uro,01/02/2023\n";
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&path, bytes).unwrap();
    let mut options = CsvOptions {
        types: Some(vec![DataType::Utf8, DataType::Date32]),
        date_format: Some("%d/%m/%Y".into()),
        ..CsvOptions::default()
    };
    let (doc, report) = import_csv(&path, &options, usize::MAX, &mut |_| true).unwrap();
    assert!(report.failures.is_empty(), "{report:?}");
    let cells = crate::display::format_batch(&doc.read(0..2).unwrap()).unwrap();
    assert_eq!(cells[0][0].as_deref(), Some("zoë"));
    assert_eq!(cells[1][0].as_deref(), Some("€uro"));
    assert_eq!(cells[0][1].as_deref(), Some("2024-12-31"));

    // Windows-1252: 0x80 is €.
    let path = dir.join("w.csv");
    std::fs::write(&path, b"a\n\x80 5\n").unwrap();
    options = CsvOptions {
        encoding: TextEncoding::Windows1252,
        ..CsvOptions::default()
    };
    let (doc, _) = import_csv(&path, &options, usize::MAX, &mut |_| true).unwrap();
    let cells = crate::display::format_batch(&doc.read(0..1).unwrap()).unwrap();
    assert_eq!(cells[0][0].as_deref(), Some("€ 5"));

    // A key that first appears after the inference window is kept.
    let path = dir.join("late.ndjson");
    let mut lines: String = (0..1500).map(|i| format!("{{\"a\": {i}}}\n")).collect();
    lines.push_str("{\"a\": 1, \"late\": \"x\"}\n");
    std::fs::write(&path, lines).unwrap();
    let doc = import_json(&path, usize::MAX, &mut |_| true).unwrap();
    assert_eq!(doc.num_columns(), 2);
    let last = crate::display::format_batch(&doc.read(1500..1501).unwrap()).unwrap();
    assert_eq!(last[0][1].as_deref(), Some("x"));
}

#[test]
fn utf16_split_across_chunks() {
    // A surrogate pair split between two decode calls.
    let text = "a😀b";
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let (first, rest) = decode_utf16(&bytes[..4], true, false);
    let mut all = rest;
    all.extend_from_slice(&bytes[4..]);
    let (second, rest) = decode_utf16(&all, true, true);
    assert!(rest.is_empty());
    let joined = String::from_utf8([first, second].concat()).unwrap();
    assert_eq!(joined, text);
}
