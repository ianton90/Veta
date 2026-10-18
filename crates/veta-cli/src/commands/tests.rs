//! Tests for the subcommands, run in-process.

#![allow(clippy::single_range_in_vec_init)] // Row ranges.

use super::parse_rows;
use crate::*;
use clap::CommandFactory;
use veta_testkit::{TempDir, fixtures};

fn parse_args(args: &[&str]) -> Command {
    Cli::try_parse_from(args).unwrap().command
}

fn output(command: Command) -> String {
    let mut out = Vec::new();
    execute(command, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}

#[test]
fn parses_subcommands() {
    assert_eq!(
        parse_args(&["veta-cli", "head", "a.parquet", "-n", "3"]),
        Command::Head {
            file: "a.parquet".into(),
            rows: 3
        }
    );
    assert!(
        Cli::try_parse_from(["veta-cli"]).is_err(),
        "a subcommand is required"
    );
    assert!(Cli::try_parse_from(["veta-cli", "a.parquet"]).is_err());
}

#[test]
fn memory_option_reads_sizes_and_shares() {
    let memory = |value: &str| {
        Cli::try_parse_from(["veta-cli", "info", "a.parquet", "--memory", value]).map(|c| c.memory)
    };
    assert_eq!(memory("512M").unwrap(), Some(512 * 1024 * 1024));
    assert!(memory("25%").unwrap().is_some());
    assert!(memory("lots").is_err());
    let none = Cli::try_parse_from(["veta-cli", "info", "a.parquet"]).unwrap();
    assert_eq!(none.memory, None);
}

#[test]
fn info_shows_layout_settings_and_metadata() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);

    let text = output(Command::Info { file });
    assert!(text.contains("Rows:        1000"), "{text}");
    assert!(text.contains("Row groups:  4 (max 300 rows)"), "{text}");
    assert!(text.contains("owner = veta-tests"), "{text}");
    assert!(text.contains("delta_binary_packed"), "{text}");
    assert!(text.contains("zstd"), "{text}");
}

#[test]
fn schema_lists_fields() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);

    let text = output(Command::Schema { file });
    assert_eq!(
        text,
        "id: int64 (required)\nname: string\nscore: float64\nflag: bool\n"
    );
}

#[test]
fn head_prints_requested_rows() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);

    let text = output(Command::Head { file, rows: 3 });
    let lines: Vec<&str> = text.lines().collect();
    // header, types, separator, 3 rows, footer
    assert_eq!(lines.len(), 7, "{text}");
    assert!(lines[0].contains("id") && lines[0].contains("name"));
    assert!(lines[1].contains("int64"));
    assert!(lines[3].contains("null"), "row 0 has a null name: {text}");
    assert_eq!(lines[6], "(3 of 1000 rows)");
}

fn run_args(args: &[&str]) -> Result<String, String> {
    let command = parse_args(args);
    let mut out = Vec::new();
    execute(command, &mut out).map_err(|e| e.to_string())?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn stats_table_and_json() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let f = file.to_str().unwrap();
    let table = run_args(&["veta", "stats", f]).unwrap();
    let lines: Vec<&str> = table.lines().collect();
    assert!(lines[0].starts_with("column"), "{table}");
    assert_eq!(lines.len(), 2 + 4, "{table}");
    let id = lines.iter().find(|l| l.starts_with("id ")).unwrap();
    assert!(
        id.contains("1000") && id.contains("499.5") && id.contains("999"),
        "{id}"
    );

    let json = run_args(&["veta", "stats", f, "-c", "name", "--json"]).unwrap();
    assert!(
        json.contains("\"column\": \"name\"") && json.contains("\"distinct\": 5"),
        "{json}"
    );
    assert!(run_args(&["veta", "stats", f, "-c", "nope"]).is_err());
}

#[test]
fn convert_between_formats() {
    let dir = TempDir::new();
    let parquet = dir.join("mixed.parquet");
    fixtures::mixed_settings(&parquet);
    let p = |path: &std::path::Path| path.to_str().unwrap().to_owned();
    let csv = dir.join("eu.csv");
    let out = run_args(&[
        "veta",
        "convert",
        &p(&parquet),
        &p(&csv),
        "--delimiter",
        ";",
        "--decimal",
        ",",
    ])
    .unwrap();
    assert!(out.starts_with("Wrote 1000 rows"), "{out}");
    assert!(std::fs::read_to_string(&csv).unwrap().contains(";0,5;"));

    let json = dir.join("out.ndjson");
    run_args(&[
        "veta",
        "convert",
        &p(&csv),
        &p(&json),
        "--delimiter",
        ";",
        "--decimal",
        ",",
    ])
    .unwrap();
    let back = dir.join("back.parquet");
    run_args(&["veta", "convert", &p(&json), &p(&back)]).unwrap();
    let original = run_args(&["veta", "head", &p(&parquet), "-n", "5"]).unwrap();
    let round_trip = run_args(&["veta", "head", &p(&back), "-n", "5"]).unwrap();
    assert_eq!(original, round_trip);

    let bad = dir.join("bad.csv");
    // Types are guessed from the first 1000 rows; a late value doesn't fit.
    let mut text = String::from("n\n");
    for i in 0..1100 {
        text.push_str(&format!("{i}\n"));
    }
    text.push_str("three\n");
    std::fs::write(&bad, text).unwrap();
    let out = run_args(&["veta", "convert", &p(&bad), &p(&dir.join("bad.parquet"))]).unwrap();
    assert!(
        out.contains("1 values didn't fit") && out.contains("n (1)"),
        "{out}"
    );
    let head = run_args(&["veta", "schema", &p(&dir.join("bad.parquet"))]).unwrap();
    assert!(head.contains("n errors"), "{head}");
    let err = run_args(&[
        "veta",
        "convert",
        &p(&bad),
        &p(&dir.join("bad2.parquet")),
        "--on-error",
        "fail",
    ]);
    assert!(err.unwrap_err().contains("\"three\""));

    let piped = dir.join("p.csv");
    run_args(&[
        "veta",
        "convert",
        &p(&parquet),
        &p(&piped),
        "--delimiter",
        "|",
        "--quote",
        "'",
    ])
    .unwrap();
    assert!(
        std::fs::read_to_string(&piped)
            .unwrap()
            .starts_with("id|name|score|flag")
    );
    assert!(run_args(&["veta", "convert", &p(&parquet), &p(&piped), "--quote", "ab"]).is_err());

    let err = run_args(&["veta", "convert", &p(&parquet), &p(&dir.join("x.xyz"))]);
    assert!(err.unwrap_err().contains("--format"));
    run_args(&[
        "veta",
        "convert",
        &p(&parquet),
        &p(&dir.join("x.xyz")),
        "--format",
        "json",
    ])
    .unwrap();
}

#[test]
fn apply_runs_a_recipe() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let recipe = dir.join("r.json");
    std::fs::write(
        &recipe,
        r#"{
  "format": "veta-recipe",
  "version": 1,
  "steps": [
{ "order": 1, "type": "filter", "settings": {
    "match_all": true,
    "conditions": [{ "column": "name", "op": "equals", "value": "ana" }] } },
{ "order": 2, "type": "sort", "settings": { "keys": [{ "column": "id", "descending": true }] } },
{ "order": 3, "type": "custom_column", "settings": { "name": "twice", "expression": "id * 2", "at": 1 } }
  ]
}"#,
    )
    .unwrap();
    let out = dir.join("out.parquet");
    let message = run_args(&[
        "veta",
        "apply",
        recipe.to_str().unwrap(),
        file.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap();
    assert!(message.starts_with("Applied 3 steps"), "{message}");
    let head = run_args(&["veta", "head", out.to_str().unwrap(), "-n", "1"]).unwrap();
    assert!(head.contains("995") && head.contains("1990"), "{head}");

    // A last sort is written as it merges.
    let text = std::fs::read_to_string(&recipe).unwrap();
    let start = text.find("{ \"order\": 3").unwrap();
    let end = text.rfind(']').unwrap();
    std::fs::write(
        &recipe,
        format!(
            "{}{}",
            text[..start].trim_end().trim_end_matches(','),
            &text[end..]
        ),
    )
    .unwrap();
    let message = run_args(&[
        "veta",
        "apply",
        recipe.to_str().unwrap(),
        file.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ])
    .unwrap();
    assert!(message.starts_with("Applied 2 steps"), "{message}");
    let head = run_args(&["veta", "head", out.to_str().unwrap(), "-n", "1"]).unwrap();
    assert!(head.contains("995") && !head.contains("1990"), "{head}");

    let one_step =
        |step: &str| format!(r#"{{"format": "veta-recipe", "version": 1, "steps": [{step}]}}"#);
    // A step that doesn't apply stops before saving.
    std::fs::write(
        &recipe,
        one_step(r#"{"order": 1, "type": "remove_columns", "settings": {"names": ["nope"]}}"#),
    )
    .unwrap();
    let err = run_args(&[
        "veta",
        "apply",
        recipe.to_str().unwrap(),
        file.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(err.contains("step 1"), "{err}");

    // A cast whose error column isn't empty stops at the check.
    std::fs::write(
        &recipe,
        r#"{"format": "veta-recipe", "version": 1, "steps": [
          {"order": 1, "type": "cast", "settings": {"column": "name", "type": "int64", "on_error": "error_column"}},
          {"order": 2, "type": "remove_empty_column", "settings": {"name": "name errors"}}
        ]}"#,
    )
    .unwrap();
    let err = run_args(&[
        "veta",
        "apply",
        recipe.to_str().unwrap(),
        file.to_str().unwrap(),
    ])
    .unwrap_err();
    assert!(
        err.contains("stopped at step 2") && err.contains("isn't empty"),
        "{err}"
    );
}

#[test]
fn edit_commands_change_the_file() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let f = file.to_str().unwrap();

    run_args(&[
        "veta", "set-cell", f, "--row", "1", "--column", "name", "--value", "first",
    ])
    .unwrap();
    run_args(&[
        "veta", "set-cell", f, "--row", "2", "--column", "score", "--null",
    ])
    .unwrap();
    run_args(&["veta", "insert-rows", f, "--at", "1001", "--count", "2"]).unwrap();
    run_args(&["veta", "delete-rows", f, "--rows", "3-4,10"]).unwrap();
    run_args(&[
        "veta",
        "add-column",
        f,
        "note",
        "--type",
        "string",
        "--at",
        "1",
    ])
    .unwrap();
    run_args(&["veta", "rename-column", f, "flag", "ok"]).unwrap();
    run_args(&["veta", "move-column", f, "ok", "--to", "2"]).unwrap();
    run_args(&["veta", "remove-column", f, "score"]).unwrap();

    let doc = Document::open(&file, OpenOptions::default()).unwrap();
    assert_eq!(doc.num_rows(), 1000 + 2 - 3);
    let names: Vec<_> = doc
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["note", "ok", "id", "name"]);
    let head = output(Command::Head {
        file: file.clone(),
        rows: 2,
    });
    assert!(head.contains("first"), "{head}");
}

#[test]
fn rows_at_top_bottom_and_ranges() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let f = file.to_str().unwrap();
    let rows = || {
        Document::open(&file, OpenOptions::default())
            .unwrap()
            .num_rows()
    };
    let first_id = || {
        let doc = Document::open(&file, OpenOptions::default()).unwrap();
        veta_core::display::format_batch(&doc.read(0..1).unwrap()).unwrap()[0][0].clone()
    };

    run_args(&["veta", "insert-rows", f, "--top", "--count", "3"]).unwrap();
    assert_eq!((rows(), first_id()), (1003, None));
    run_args(&["veta", "insert-rows", f, "--bottom"]).unwrap();
    assert_eq!(rows(), 1004);
    run_args(&["veta", "delete-rows", f, "--top", "3"]).unwrap();
    assert_eq!((rows(), first_id().as_deref()), (1001, Some("0")));
    run_args(&["veta", "remove-rows", f, "--bottom", "1"]).unwrap();
    assert_eq!(rows(), 1000);
    run_args(&["veta", "keep-rows", f, "--rows", "11-20,31-40"]).unwrap();
    assert_eq!((rows(), first_id().as_deref()), (20, Some("10")));
    run_args(&["veta", "keep-rows", f, "--bottom", "5"]).unwrap();
    assert_eq!((rows(), first_id().as_deref()), (5, Some("35")));
    run_args(&["veta", "keep-rows", f, "--top", "2"]).unwrap();
    assert_eq!(rows(), 2);

    let usage = |args: &[&str]| Cli::try_parse_from(args).is_err();
    assert!(usage(&["veta", "insert-rows", f]), "needs a place");
    assert!(usage(&["veta", "insert-rows", f, "--top", "--bottom"]));
    assert!(usage(&["veta", "keep-rows", f]));
    assert!(
        run_args(&["veta", "delete-rows", f, "--top", "3"]).is_err(),
        "only 2"
    );
    assert!(run_args(&["veta", "keep-rows", f, "--rows", "1-5"]).is_err());
    assert!(run_args(&["veta", "insert-rows", f, "--at", "4"]).is_err());
    assert_eq!(rows(), 2);
}

#[test]
fn edit_to_output_leaves_source_alone() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let copy = dir.join("copy.parquet");
    let text = run_args(&[
        "veta",
        "delete-rows",
        file.to_str().unwrap(),
        "--rows",
        "1-500",
        "-o",
        copy.to_str().unwrap(),
    ])
    .unwrap();
    assert!(text.starts_with("Saved"));
    assert_eq!(
        Document::open(&file, OpenOptions::default())
            .unwrap()
            .num_rows(),
        1000
    );
    assert_eq!(
        Document::open(&copy, OpenOptions::default())
            .unwrap()
            .num_rows(),
        500
    );
}

#[test]
fn meta_commands() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let f = file.to_str().unwrap();
    run_args(&["veta", "meta", "set", f, "team", "data"]).unwrap();
    assert_eq!(
        run_args(&["veta", "meta", "get", f, "team"]).unwrap(),
        "data\n"
    );
    run_args(&["veta", "meta", "remove", f, "owner"]).unwrap();
    let list = run_args(&["veta", "meta", "list", f]).unwrap();
    assert!(
        list.contains("team = data") && !list.contains("owner"),
        "{list}"
    );
    assert!(run_args(&["veta", "meta", "get", f, "owner"]).is_err());

    // Column metadata.
    run_args(&["veta", "meta", "set", f, "unit", "m", "--column", "score"]).unwrap();
    run_args(&["veta", "meta", "set", f, "note", "x", "-c", "score"]).unwrap();
    assert_eq!(
        run_args(&["veta", "meta", "get", f, "unit", "-c", "score"]).unwrap(),
        "m\n"
    );
    run_args(&["veta", "meta", "remove", f, "note", "-c", "score"]).unwrap();
    assert_eq!(
        run_args(&["veta", "meta", "list", f, "-c", "score"]).unwrap(),
        "unit = m\n"
    );
    assert!(run_args(&["veta", "meta", "list", f, "-c", "missing"]).is_err());
    assert!(
        !run_args(&["veta", "meta", "list", f])
            .unwrap()
            .contains("unit")
    );
}

#[test]
fn writer_settings_command() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let f = file.to_str().unwrap();
    let shown = run_args(&["veta", "writer-settings", f]).unwrap();
    assert!(shown.contains("300 rows per row group"), "{shown}");

    run_args(&[
        "veta",
        "writer-settings",
        f,
        "--column",
        "name",
        "--compression",
        "zstd",
        "--level",
        "5",
        "--encoding",
        "delta-byte-array",
        "--dictionary",
        "off",
        "--row-group-rows",
        "100",
    ])
    .unwrap();
    let doc = Document::open(&file, OpenOptions::default()).unwrap();
    let name = doc.writer_settings().column("name");
    assert!(matches!(name.compression, veta_core::Compression::Zstd(_)));
    assert_eq!(name.encoding, veta_core::Encoding::DeltaByteArray);
    assert_eq!(doc.file_info().unwrap().row_groups.len(), 10);

    let err = run_args(&[
        "veta",
        "writer-settings",
        f,
        "--column",
        "flag",
        "--encoding",
        "delta-byte-array",
    ])
    .unwrap_err();
    assert!(err.contains("does not apply"), "{err}");
    assert!(run_args(&["veta", "writer-settings", f, "--level", "3"]).is_err());
}

#[test]
fn edit_errors_are_reported() {
    let dir = TempDir::new();
    let file = dir.join("mixed.parquet");
    fixtures::mixed_settings(&file);
    let f = file.to_str().unwrap();
    let err = run_args(&[
        "veta", "set-cell", f, "--row", "1", "--column", "score", "--value", "abc",
    ])
    .unwrap_err();
    assert!(err.contains("not a valid float64"), "{err}");
    assert!(
        run_args(&[
            "veta", "set-cell", f, "--row", "0", "--column", "id", "--value", "1"
        ])
        .is_err()
    );
    assert!(run_args(&["veta", "add-column", f, "x", "--type", "colour"]).is_err());
    assert!(run_args(&["veta", "remove-column", f, "nope"]).is_err());
    // Nothing was written by the failed commands.
    assert!(
        !Document::open(&file, OpenOptions::default())
            .unwrap()
            .is_modified()
    );
    assert_eq!(
        output(Command::Head {
            file: file.clone(),
            rows: 1
        })
        .lines()
        .count(),
        5
    );
}

#[test]
fn missing_file_error_names_the_file() {
    let mut out = Vec::new();
    let err = execute(
        Command::Info {
            file: "/nonexistent/x.parquet".into(),
        },
        &mut out,
    )
    .unwrap_err();
    assert!(err.to_string().contains("/nonexistent/x.parquet"));
}

#[test]
fn row_specs() {
    let specs = vec!["5".to_owned(), "10-12, 1".to_owned()];
    assert_eq!(parse_rows(&specs).unwrap(), vec![4..5, 9..12, 0..1]);
    assert!(parse_rows(&["0".to_owned()]).is_err());
    assert!(parse_rows(&["5-3".to_owned()]).is_err());
    assert!(parse_rows(&["x".to_owned()]).is_err());
}
