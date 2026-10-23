//! Recipes: steps saved on one file and applied to another.

#![allow(clippy::unwrap_used, clippy::single_range_in_vec_init)] // Tests.

use veta_core::controller::{execute, undo};
use veta_core::display::format_batch;
use veta_core::recipe::{RecipeInfo, from_json, to_json};
use veta_core::steps::Step;
use veta_core::steps::{Condition, CustomColumn, Filter, FilterOp, Sort, SortKey, Status};
use veta_core::{Command, Document, OpenOptions};
use veta_testkit::{TempDir, fixtures};

fn all(doc: &Document) -> Vec<Vec<Option<String>>> {
    format_batch(&doc.read(0..doc.num_rows()).unwrap()).unwrap()
}

fn steps() -> Vec<Command> {
    vec![
        Command::AddStep(Step::RenameColumn {
            from: "name".into(),
            to: "label".into(),
        }),
        Command::AddStep(Step::CustomColumn(CustomColumn {
            name: "double".into(),
            expression: "id * 2".into(),
            at: 1,
        })),
        Command::AddStep(Step::Filter(Filter {
            conditions: vec![Condition {
                column: "flag".into(),
                op: FilterOp::Equals,
                value: "true".into(),
                case_sensitive: false,
            }],
            match_all: true,
            exclude: false,
        })),
        Command::AddStep(Step::Sort(Sort {
            keys: vec![SortKey {
                column: "score".into(),
                descending: true,
                nulls_first: false,
            }],
        })),
    ]
}

fn run(doc: &mut Document, commands: Vec<Command>) {
    for command in commands {
        execute(doc, command).unwrap();
        doc.compute_all().unwrap();
    }
}

#[test]
fn recipe_from_one_file_applies_to_another() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    // The other file: same columns, different rows.
    let other_path = dir.join("other.parquet");
    let mut other = Document::open(&path, OpenOptions::default()).unwrap();
    execute(
        &mut other,
        Command::AddStep(Step::DeleteRows { rows: vec![0..137] }),
    )
    .unwrap();
    other.save(Some(other_path.clone())).unwrap();

    let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
    run(&mut doc, steps());
    let info = RecipeInfo {
        name: "tidy".into(),
        source_file: Some("mixed.parquet".into()),
    };
    let recipe_path = dir.join("tidy.json");
    std::fs::write(&recipe_path, to_json(doc.steps(), &info).unwrap()).unwrap();

    for options in [OpenOptions::default(), OpenOptions::paged()] {
        let mut target = Document::open(&other_path, options).unwrap();
        let text = std::fs::read_to_string(&recipe_path).unwrap();
        let steps = from_json(&text, &target.schema()).unwrap();
        assert_eq!(steps.len(), 4);
        execute(&mut target, Command::AppendSteps { steps }).unwrap();
        target.compute_all().unwrap();
        assert_eq!(target.status(), &Status::Ready);

        let mut by_hand = Document::open(&other_path, options).unwrap();
        run(&mut by_hand, self::steps());
        assert_eq!(all(&target), all(&by_hand));
        assert!(target.num_rows() < doc.num_rows());
        // The whole recipe is one undo.
        assert!(undo(&mut target));
        assert!(target.steps().is_empty());
    }
}

#[test]
fn recipe_on_a_file_without_its_columns_breaks() {
    let dir = TempDir::new();
    let path = dir.join("mixed.parquet");
    fixtures::mixed_settings(&path);
    let mut doc = Document::open(&path, OpenOptions::default()).unwrap();
    run(&mut doc, steps());
    let text = to_json(doc.steps(), &RecipeInfo::default()).unwrap();

    let other_path = dir.join("large.parquet");
    fixtures::large(&other_path, 100, 50);
    let mut other = Document::open(&other_path, OpenOptions::default()).unwrap();
    let applied = from_json(&text, &other.schema()).and_then(|steps| {
        execute(&mut other, Command::AppendSteps { steps })?;
        other.compute_all()
    });
    let broken = matches!(other.status(), Status::Broken { .. });
    assert!(applied.is_err() || broken, "a missing column is reported");
    assert!(from_json("{\"format\": \"other\"}", &other.schema()).is_err());
}
