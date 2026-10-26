//! Undo and redo, editing steps, cell edits and progress, through the
//! `DataFrame` API.

#![allow(clippy::unwrap_used)] // Tests.

use std::sync::Arc;

use arrow::array::{Array, AsArray, Int64Array};
use arrow::datatypes::{DataType, Field, Int64Type, Schema};
use arrow::record_batch::RecordBatch;
use veta_core::error::Error;
use veta_core::{DataFrame, Null, Progress, read_parquet};
use veta_testkit::{TempDir, fixtures};

fn ids(frame: &DataFrame) -> Vec<i64> {
    let values = frame.column("id").unwrap().values().unwrap();
    values.as_primitive::<Int64Type>().values().to_vec()
}

fn frame(dir: &TempDir, rows: usize) -> DataFrame {
    let path = dir.path().join("large.parquet");
    fixtures::large(&path, rows, 10_000);
    read_parquet(&path).unwrap().with_history(true)
}

#[test]
fn undo_and_redo() {
    let dir = TempDir::new();
    let df = frame(&dir, 1000);
    assert!(!df.can_undo());
    let a = df.keep_rows(0..10).unwrap();
    let b = a.drop_columns(["label"]).unwrap();
    assert_eq!(b.num_columns(), 2);
    let back = b.undo().unwrap();
    assert_eq!((back.num_columns(), back.num_rows().unwrap()), (3, 10));
    let back = back.undo().unwrap();
    assert_eq!(back.num_rows().unwrap(), 1000);
    assert!(!back.can_undo() && back.can_redo());
    let again = back.redo().unwrap().redo().unwrap();
    assert_eq!((again.num_columns(), again.num_rows().unwrap()), (2, 10));
    // A change after an undo drops what could be redone.
    let other = b.undo().unwrap().keep_rows(0..5).unwrap();
    assert!(!other.can_redo());
    assert!(matches!(other.redo(), Err(Error::Invalid(_))));
    // Without history nothing is kept.
    let off = b.with_history(false);
    assert!(!off.can_undo() && off.undo().is_err());
}

#[test]
fn editing_moving_and_removing_steps() {
    let dir = TempDir::new();
    let df = frame(&dir, 1000);
    let df = df
        .keep_rows(0..100)
        .unwrap()
        .drop_rows(0..90)
        .unwrap()
        .drop_columns(["value"])
        .unwrap();
    assert_eq!(df.num_steps(), 4);
    assert_eq!(ids(&df), (90..100).collect::<Vec<_>>());
    // Step 1 (keep 0..100) becomes keep 500..600: the later steps run on it.
    let edited = df
        .edit_step(1, |before| before.keep_rows(500..600))
        .unwrap();
    assert_eq!(ids(&edited), (590..600).collect::<Vec<_>>());
    assert_eq!(ids(&edited.undo().unwrap()), ids(&df));
    let removed = df.remove_step(2).unwrap();
    assert_eq!(ids(&removed), (0..100).collect::<Vec<_>>());
    assert_eq!(removed.columns(), ["id", "label"]);
    let moved = df.move_step(3, 1).unwrap();
    assert_eq!(moved.columns(), ["id", "label"]);
    assert_eq!(ids(&moved), (90..100).collect::<Vec<_>>());
    // A later step that no longer fits is kept, broken.
    let broken = df.edit_step(1, |before| before.keep_rows(0..50)).unwrap();
    assert_eq!(broken.num_steps(), 4);
    let (step, error) = broken.broken_step().unwrap();
    assert_eq!(step, 2);
    assert!(matches!(&error, Error::Invalid(e) if e.arg("step") == Some("2")));
    assert!(broken.num_rows().is_err() && broken.rows(0..1).is_err());
    assert!(broken.keep_rows(0..1).is_err(), "fix or remove it first");
    assert!(broken.set(0, "id", 1).is_err());
    // Removing it, or editing what comes before, plans the rest again.
    let fixed = broken.remove_step(2).unwrap();
    assert!(fixed.broken_step().is_none());
    assert_eq!(fixed.columns(), ["id", "label"]);
    assert_eq!(ids(&fixed).len(), 50);
    let fixed = broken
        .edit_step(1, |before| before.keep_rows(0..95))
        .unwrap();
    assert_eq!(ids(&fixed), (90..95).collect::<Vec<_>>());
    assert!(broken.undo().unwrap().broken_step().is_none());
    assert!(df.remove_step(9).is_err());
    assert!(df.edit_step(1, |_| Ok(DataFrame::new())).is_err());
}

#[test]
fn cell_edits_undo_one_by_one() {
    let dir = TempDir::new();
    let df = frame(&dir, 1000);
    let edited = df
        .set(5, "id", 500)
        .unwrap()
        .set(6, "id", 600)
        .unwrap()
        .set(5, "label", "five")
        .unwrap();
    assert_eq!(edited.num_steps(), 2);
    assert_eq!(ids(&edited)[4..8], [4, 500, 600, 7]);
    let label = |frame: &DataFrame| {
        frame
            .value(5, "label")
            .unwrap()
            .as_string::<i32>()
            .value(0)
            .to_owned()
    };
    assert_eq!(label(&edited), "five");
    let one_back = edited.undo().unwrap();
    assert_eq!(label(&one_back), "row-5");
    assert_eq!(ids(&one_back.undo().unwrap())[4..8], [4, 500, 6, 7]);
    // After an undo, a new edit doesn't change the frames made before.
    let branch = edited
        .undo()
        .unwrap()
        .undo()
        .unwrap()
        .set(7, "id", 700)
        .unwrap();
    assert_eq!(ids(&branch)[4..8], [4, 500, 6, 700]);
    assert_eq!(ids(&edited)[4..8], [4, 500, 600, 7]);
    // Values are converted to the column's type, or refused.
    assert!(df.set(0, "id", "12").is_ok());
    assert!(df.set(0, "id", "twelve").is_err());
    assert!(df.set(0, "id", Null).is_err(), "id isn't nullable");
    let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, true)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
    )
    .unwrap();
    let small = DataFrame::from_batches(schema, vec![batch]).unwrap();
    assert!(
        small
            .set(1, "n", Null)
            .unwrap()
            .value(1, "n")
            .unwrap()
            .is_null(0)
    );
    assert!(df.set(1000, "id", 1).is_err());
}

#[test]
fn many_cell_edits_are_grouped() {
    let dir = TempDir::new();
    let df = frame(&dir, 1000);
    veta_core::memory::set_cell_edits_limit(Some(16 * 1024));
    let mut edited = df.clone();
    for row in 0..200 {
        edited = edited.set(row, "id", -(row as i64)).unwrap();
    }
    veta_core::memory::set_cell_edits_limit(None);
    let ids_now = ids(&edited);
    assert_eq!(ids_now[..3], [0, -1, -2]);
    assert_eq!(ids_now[199..201], [-199, 200]);
    // The grouped edits are undone as one, back past the first of them.
    let mut back = edited.clone();
    let mut undos = 0;
    while back.can_undo() {
        back = back.undo().unwrap();
        undos += 1;
    }
    assert!(undos < 200, "{undos} undos");
    assert_eq!(ids(&back), (0..1000).collect::<Vec<_>>());
}

#[test]
fn progress_and_cancel() {
    let dir = TempDir::new();
    let df = frame(&dir, 300_000);
    let progress = Progress::new();
    let df = df.with_progress(&progress);
    df.write_parquet(dir.path().join("out.parquet")).unwrap();
    assert_eq!(progress.task(), Some(veta_core::Task::Writing));
    assert_eq!(progress.fraction(), 1.0);
    progress.cancel();
    assert!(matches!(
        df.write_parquet(dir.path().join("out2.parquet")),
        Err(Error::Cancelled)
    ));
    assert!(!dir.path().join("out2.parquet").exists());
    progress.reset();
    assert!(df.column("id").unwrap().values().is_ok());
}
