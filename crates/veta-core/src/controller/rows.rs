//! Choosing rows by position for inserting, deleting and keeping them:
//! at the top, at the bottom, or at a row number. Shared by the GUI and the
//! CLI.

use crate::error::invalid;
use std::ops::Range;

use crate::command::Command;
use crate::history::Change;
use crate::steps::{Step, normalize_ranges};

use crate::error::Result;

/// Where rows go, or which rows are meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowPlace {
    Top,
    Bottom,
    /// From this row (0-based).
    At(usize),
}

/// What to do with the rows (see [`rows_command`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Insert,
    Delete,
    Keep,
}

/// The `count` existing rows at `place` in a table of `num_rows` rows.
pub fn row_range(place: RowPlace, count: usize, num_rows: usize) -> Result<Range<usize>> {
    if count == 0 {
        return Err(invalid!(RowCountZero));
    }
    let start = match place {
        RowPlace::Top => 0,
        RowPlace::Bottom => num_rows.saturating_sub(count),
        RowPlace::At(at) if at < num_rows => at,
        RowPlace::At(at) => {
            return Err(invalid!(RowPastEnd, row = at + 1, rows = num_rows));
        }
    };
    if start + count > num_rows {
        return Err(invalid!(RowsFromThere, rows = num_rows - start));
    }
    Ok(start..start + count)
}

/// The command that inserts `count` empty rows at `place` (before the row,
/// for [`RowPlace::At`]; the row count appends), or deletes or keeps only
/// the `count` rows there.
pub fn rows_command(
    action: RowAction,
    place: RowPlace,
    count: usize,
    num_rows: usize,
) -> Result<Command> {
    match action {
        RowAction::Insert => {
            if count == 0 {
                return Err(invalid!(RowCountZero));
            }
            let at = match place {
                RowPlace::Top => 0,
                RowPlace::Bottom => num_rows,
                RowPlace::At(at) if at <= num_rows => at,
                RowPlace::At(at) => {
                    return Err(invalid!(RowPastEndUse, row = at + 1, end = num_rows + 1));
                }
            };
            Ok(Command::InsertRows { at, count })
        }
        RowAction::Delete => Ok(Command::DeleteRows {
            rows: vec![row_range(place, count, num_rows)?],
        }),
        RowAction::Keep => Ok(Command::DeleteRows {
            rows: keep_only(&[row_range(place, count, num_rows)?], num_rows),
        }),
    }
}

/// Refuses ranges that go past the last of `num_rows` rows.
pub fn check_rows(ranges: &[Range<usize>], num_rows: usize) -> Result<()> {
    match ranges.iter().find(|r| r.end > num_rows) {
        Some(bad) => Err(invalid!(RowPastEnd, row = bad.end, rows = num_rows)),
        None => Ok(()),
    }
}

/// The rows to delete to keep only `keep` (sorted or not, may overlap).
pub fn keep_only(keep: &[Range<usize>], num_rows: usize) -> Vec<Range<usize>> {
    let mut keep: Vec<Range<usize>> = keep.iter().filter(|r| !r.is_empty()).cloned().collect();
    keep.sort_by_key(|r| r.start);
    let mut delete = Vec::new();
    let mut next = 0;
    for range in keep {
        if range.start > next {
            delete.push(next..range.start.min(num_rows));
        }
        next = next.max(range.end);
    }
    if next < num_rows {
        delete.push(next..num_rows);
    }
    delete.retain(|r| !r.is_empty());
    delete
}

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(command: Command) -> Result<Change> {
    match command {
        Command::InsertRows { at, count } => Ok(Change::PushStep(Step::InsertRows { at, count })),
        Command::DeleteRows { rows: ranges } => {
            let ranges = normalize_ranges(ranges);
            Ok(Change::PushStep(Step::DeleteRows { rows: ranges }))
        }
        other => Err(invalid!(
            WrongArea,
            area = "rows",
            command = format!("{other:?}")
        )),
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Row ranges, not a range of values.
mod tests {
    use super::*;

    #[test]
    fn places_and_counts() {
        use RowAction::*;
        use RowPlace::*;
        let cmd = |a, p, n| rows_command(a, p, n, 10);
        assert_eq!(
            cmd(Insert, Top, 3).unwrap(),
            Command::InsertRows { at: 0, count: 3 }
        );
        assert_eq!(
            cmd(Insert, Bottom, 2).unwrap(),
            Command::InsertRows { at: 10, count: 2 }
        );
        assert_eq!(
            cmd(Insert, At(10), 1).unwrap(),
            Command::InsertRows { at: 10, count: 1 }
        );
        assert!(cmd(Insert, At(11), 1).is_err());
        assert!(cmd(Insert, Top, 0).is_err());
        assert_eq!(
            cmd(Delete, Top, 3).unwrap(),
            Command::DeleteRows { rows: vec![0..3] }
        );
        assert_eq!(
            cmd(Delete, Bottom, 3).unwrap(),
            Command::DeleteRows { rows: vec![7..10] }
        );
        assert_eq!(
            cmd(Delete, At(4), 2).unwrap(),
            Command::DeleteRows { rows: vec![4..6] }
        );
        assert!(cmd(Delete, At(9), 2).is_err());
        assert!(cmd(Delete, Bottom, 11).is_err());
        assert_eq!(
            cmd(Keep, Top, 3).unwrap(),
            Command::DeleteRows { rows: vec![3..10] }
        );
        assert_eq!(
            cmd(Keep, Bottom, 3).unwrap(),
            Command::DeleteRows { rows: vec![0..7] }
        );
        assert_eq!(
            cmd(Keep, At(4), 2).unwrap(),
            Command::DeleteRows {
                rows: vec![0..4, 6..10]
            }
        );
        assert_eq!(keep_only(&[5..7, 0..2, 6..8], 10), vec![2..5, 8..10]);
        assert!(keep_only(&[0..10], 10).is_empty());
    }
}
