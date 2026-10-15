//! Runs the real `veta-cli` binary: output, exit codes, written files and
//! error messages for each subcommand.

#![allow(clippy::unwrap_used)] // Tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use veta_core::display::format_batch;
use veta_core::recipe::{RecipeInfo, to_json};
use veta_core::steps::{Sort, SortKey, Step};
use veta_core::{Document, OpenOptions};
use veta_testkit::{TempDir, fixtures};

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn veta(args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_veta-cli"))
        .args(args)
        .output()
        .unwrap();
    Run {
        code: output.status.code().unwrap(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Runs `args`, which must succeed, and returns the output.
fn ok(args: &[&str]) -> String {
    let run = veta(args);
    assert_eq!(run.code, 0, "{args:?} failed: {}", run.stderr);
    run.stdout
}

/// Runs `args`, which must fail with exit code 1, and returns the error.
fn fails(args: &[&str]) -> String {
    let run = veta(args);
    assert_eq!(run.code, 1, "{args:?} should fail: {}", run.stdout);
    assert!(run.stderr.starts_with("veta-cli: "), "{}", run.stderr);
    run.stderr
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A copy of the `mixed_settings` fixture (id, name, score, flag; 1000 rows).
fn mixed(dir: &TempDir, name: &str) -> PathBuf {
    let path = dir.join(name);
    fixtures::mixed_settings(&path);
    path
}

fn open(path: &Path) -> Document {
    Document::open(path, OpenOptions::default()).unwrap()
}

fn names(path: &Path) -> Vec<String> {
    open(path)
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect()
}

fn cell(path: &Path, row: usize, column: usize) -> Option<String> {
    let doc = open(path);
    format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][column].clone()
}

#[test]
fn version_help_and_usage_errors() {
    assert!(ok(&["--version"]).starts_with("veta-cli "));
    assert!(ok(&["--help"]).contains("Usage: veta-cli <COMMAND>"));
    let run = veta(&["nonsense"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("unrecognized subcommand 'nonsense'"));
    let run = veta(&["insert-rows", "f.parquet"]);
    assert_eq!(run.code, 2, "one of --top, --bottom or --at is required");
}

#[test]
fn info_schema_head_and_stats() {
    let dir = TempDir::new();
    let path = mixed(&dir, "m.parquet");
    let info = ok(&["info", s(&path)]);
    assert!(info.contains("Rows:        1000"), "{info}");
    assert!(info.contains("Columns:     4"));
    assert!(info.contains("owner = veta-tests"));
    assert!(info.contains("id     │ zstd"));

    assert_eq!(
        ok(&["schema", s(&path)]),
        "id: int64 (required)\nname: string\nscore: float64\nflag: bool\n"
    );

    let head = ok(&["head", "-n", "2", s(&path)]);
    assert!(head.contains("    1 │ bo     │     0.5 │ false"), "{head}");
    assert!(head.ends_with("(2 of 1000 rows)\n"));

    let stats = ok(&["stats", "-c", "score", s(&path)]);
    assert!(
        stats.contains("score  │ float64 │  1000 │    91"),
        "{stats}"
    );
    let json = ok(&["stats", "--json", "-c", "id", s(&path)]);
    assert!(json.contains("\"distinct\": 1000"));
    assert!(json.contains("\"max\": \"999\""));
    assert!(fails(&["stats", "-c", "nope", s(&path)]).contains("nope"));
}

#[test]
fn missing_and_invalid_files() {
    let dir = TempDir::new();
    let missing = dir.join("missing.parquet");
    let err = fails(&["head", s(&missing)]);
    assert!(err.contains("missing.parquet"), "{err}");
    let text = dir.join("text.parquet");
    std::fs::write(&text, "not parquet").unwrap();
    fails(&["info", s(&text)]);
}

#[test]
fn cell_and_row_edits() {
    let dir = TempDir::new();
    let path = mixed(&dir, "m.parquet");
    assert_eq!(
        ok(&[
            "set-cell",
            s(&path),
            "--row",
            "2",
            "--column",
            "name",
            "--value",
            "zed"
        ]),
        format!("Saved {}\n", s(&path))
    );
    assert_eq!(cell(&path, 1, 1).as_deref(), Some("zed"));
    ok(&[
        "set-cell",
        s(&path),
        "--row",
        "2",
        "--column",
        "name",
        "--null",
    ]);
    assert_eq!(cell(&path, 1, 1), None);

    ok(&["insert-rows", s(&path), "--top", "--count", "2"]);
    assert_eq!(open(&path).num_rows(), 1002);
    assert_eq!(cell(&path, 0, 0), None, "inserted rows are empty");
    ok(&["insert-rows", s(&path), "--at", "1003"]);
    assert_eq!(open(&path).num_rows(), 1003);
    ok(&["remove-rows", s(&path), "--rows", "1-2,1003"]);
    assert_eq!(open(&path).num_rows(), 1000);
    ok(&["delete-rows", s(&path), "--bottom", "100"]);
    assert_eq!(open(&path).num_rows(), 900);

    // --output leaves the file as it is.
    let kept = dir.join("kept.parquet");
    ok(&["keep-rows", s(&path), "--rows", "5-9", "-o", s(&kept)]);
    assert_eq!(open(&kept).num_rows(), 5);
    assert_eq!(cell(&kept, 0, 0).as_deref(), Some("4"));
    assert_eq!(open(&path).num_rows(), 900);

    // Errors name the problem and change nothing.
    let err = fails(&[
        "set-cell",
        s(&path),
        "--row",
        "1",
        "--column",
        "bogus",
        "--value",
        "1",
    ]);
    assert!(err.contains("No column named \"bogus\""), "{err}");
    assert!(
        fails(&[
            "set-cell",
            s(&path),
            "--row",
            "1",
            "--column",
            "id",
            "--value",
            "x"
        ])
        .contains("id")
    );
    assert!(fails(&["remove-rows", s(&path), "--rows", "0"]).contains("row"));
    assert_eq!(open(&path).num_rows(), 900);
}

#[test]
fn column_edits() {
    let dir = TempDir::new();
    let path = mixed(&dir, "m.parquet");
    ok(&[
        "add-column",
        s(&path),
        "extra",
        "--type",
        "int64",
        "--at",
        "2",
    ]);
    assert_eq!(names(&path), ["id", "extra", "name", "score", "flag"]);
    ok(&["rename-column", s(&path), "extra", "more"]);
    ok(&["move-column", s(&path), "more", "--to", "5"]);
    assert_eq!(names(&path), ["id", "name", "score", "flag", "more"]);
    ok(&["remove-column", s(&path), "more", "flag"]);
    assert_eq!(names(&path), ["id", "name", "score"]);
    assert!(fails(&["add-column", s(&path), "x", "--type", "nope"]).contains("unknown type"));
    assert!(fails(&["rename-column", s(&path), "id", "name"]).contains("name"));
}

#[test]
fn metadata_and_writer_settings() {
    let dir = TempDir::new();
    let path = mixed(&dir, "m.parquet");
    ok(&["meta", "set", s(&path), "k", "v"]);
    assert_eq!(ok(&["meta", "get", s(&path), "k"]), "v\n");
    assert!(ok(&["meta", "list", s(&path)]).contains("k = v"));
    ok(&["meta", "remove", s(&path), "k"]);
    assert!(fails(&["meta", "get", s(&path), "k"]).contains("no metadata key \"k\""));
    ok(&["meta", "set", s(&path), "unit", "kg", "-c", "score"]);
    assert_eq!(
        ok(&["meta", "get", s(&path), "unit", "-c", "score"]),
        "kg\n"
    );

    let out = dir.join("w.parquet");
    ok(&[
        "writer-settings",
        s(&path),
        "--compression",
        "zstd",
        "--level",
        "3",
        "-o",
        s(&out),
    ]);
    let settings = ok(&["writer-settings", s(&out)]);
    for column in ["id", "name", "score", "flag"] {
        let line = settings.lines().find(|l| l.starts_with(column)).unwrap();
        assert!(line.contains("zstd"), "{line}");
    }
}

#[test]
fn convert_round_trips() {
    let dir = TempDir::new();
    let path = mixed(&dir, "m.parquet");
    let csv = dir.join("m.csv");
    assert_eq!(
        ok(&["convert", s(&path), s(&csv)]),
        format!("Wrote 1000 rows to {}\n", s(&csv))
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("id,name,score,flag\n0,,,true\n1,bo,0.5,false\n"));
    let back = dir.join("back.parquet");
    ok(&["convert", s(&csv), s(&back)]);
    let all = |p: &Path| format_batch(&open(p).read(0..1000).unwrap()).unwrap();
    assert_eq!(all(&back), all(&path));

    let tsv = dir.join("m.txt");
    ok(&[
        "convert",
        s(&path),
        s(&tsv),
        "--format",
        "csv",
        "--delimiter",
        ";",
    ]);
    assert!(
        std::fs::read_to_string(&tsv)
            .unwrap()
            .starts_with("id;name;score;flag\n")
    );
    let json = dir.join("m.jsonl");
    ok(&["convert", s(&path), s(&json)]);
    let from_json = dir.join("j.parquet");
    ok(&["convert", s(&json), s(&from_json)]);
    assert_eq!(all(&from_json), all(&path));
    assert!(fails(&["convert", s(&path), s(&dir.join("m.xyz"))]).contains("format"));
}

#[test]
fn apply_a_recipe() {
    let dir = TempDir::new();
    let path = mixed(&dir, "m.parquet");
    let steps = vec![
        Step::RenameColumn {
            from: "name".into(),
            to: "label".into(),
        },
        Step::Sort(Sort {
            keys: vec![SortKey {
                column: "id".into(),
                descending: true,
                nulls_first: false,
            }],
        }),
    ];
    let recipe = dir.join("r.json");
    std::fs::write(&recipe, to_json(&steps, &RecipeInfo::default()).unwrap()).unwrap();
    let out = dir.join("out.parquet");
    let printed = ok(&["apply", s(&recipe), s(&path), "-o", s(&out)]);
    assert_eq!(printed, format!("Applied 2 steps; saved {}\n", s(&out)));
    assert_eq!(names(&out), ["id", "label", "score", "flag"]);
    assert_eq!(cell(&out, 0, 0).as_deref(), Some("999"));

    std::fs::write(&recipe, "{}").unwrap();
    fails(&["apply", s(&recipe), s(&path)]);
}
