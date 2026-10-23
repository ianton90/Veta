//! The steps that insert or delete rows, and their row range helpers.

use std::ops::Range;

use arrow::record_batch::RecordBatch;

use super::MAX_INSERT_ROWS;
use super::level::Level;
use super::read::{concat, empty_rows};
use super::transform::{Input, Transform};
use crate::error::{Result, invalid};

/// [`Step::InsertRows`](super::Step::InsertRows).
pub(super) struct InsertRows {
    pub(super) at: usize,
    pub(super) count: usize,
}

impl Transform for InsertRows {
    fn positional(&self) -> bool {
        true
    }

    fn level(&self, input: &Level) -> Result<Level> {
        let (at, count) = (self.at, self.count);
        if at > input.num_rows {
            return Err(invalid!(InsertPastEnd, row = input.num_rows));
        }
        if count == 0 {
            return Err(invalid!(NothingToInsert));
        }
        let too_many = || invalid!(InsertTooMany, max = MAX_INSERT_ROWS);
        if count > MAX_INSERT_ROWS {
            return Err(too_many());
        }
        let num_rows = input.num_rows.checked_add(count).ok_or_else(too_many)?;
        // Inserted rows are empty, so every column must allow nulls.
        let fields = input
            .fields()
            .into_iter()
            .map(|f| f.with_nullable(true))
            .collect();
        Ok(input.with_fields(fields, num_rows))
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        let (at, count) = (self.at, self.count);
        let schema = input.output_schema(names);
        let mut parts = Vec::new();
        let before = range.start..range.end.min(at);
        if !before.is_empty() {
            parts.push(input.read(before, names)?);
        }
        let inserted = range.start.max(at)..range.end.min(at + count);
        if !inserted.is_empty() {
            parts.push(empty_rows(&schema, inserted.len())?);
        }
        let after = range.start.max(at + count)..range.end;
        if !after.is_empty() {
            parts.push(input.read(after.start - count..after.end - count, names)?);
        }
        concat(&schema, parts)
    }
}

/// [`Step::DeleteRows`](super::Step::DeleteRows).
pub(super) struct DeleteRows<'a> {
    pub(super) rows: &'a [Range<usize>],
}

impl Transform for DeleteRows<'_> {
    fn positional(&self) -> bool {
        true
    }

    fn level(&self, input: &Level) -> Result<Level> {
        if self.rows.is_empty() {
            return Err(invalid!(NoRowsToDelete));
        }
        let mut previous_end = 0;
        for (i, r) in self.rows.iter().enumerate() {
            if r.is_empty() || (i > 0 && r.start <= previous_end) {
                return Err(invalid!(RowRanges));
            }
            if r.end > input.num_rows {
                return Err(invalid!(RowPastEnd, row = r.end, rows = input.num_rows));
            }
            previous_end = r.end;
        }
        let deleted: usize = self.rows.iter().map(|r| r.len()).sum();
        Ok(Level {
            schema: input.schema.clone(),
            num_rows: input.num_rows - deleted,
        })
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        let parts = kept_ranges(self.rows, range)
            .into_iter()
            .map(|r| input.read(r, names))
            .collect::<Result<Vec<_>>>()?;
        concat(&input.output_schema(names), parts)
    }
}

/// Maps an output row range of a delete step to the previous step's row
/// ranges that survive.
pub(super) fn kept_ranges(deleted: &[Range<usize>], output: Range<usize>) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    // Walk kept segments of the previous level, tracking their output offset.
    let mut prev_start = 0; // start of the current kept segment (previous level)
    let mut out_start = 0; // its position in the output
    let push = |segment: Range<usize>, out_offset: usize, result: &mut Vec<Range<usize>>| {
        let seg_out = out_offset..out_offset + segment.len();
        let from = output.start.max(seg_out.start);
        let to = output.end.min(seg_out.end);
        if from < to {
            let shift = segment.start as isize - seg_out.start as isize;
            result.push((from as isize + shift) as usize..(to as isize + shift) as usize);
        }
    };
    for d in deleted {
        let segment = prev_start..d.start;
        if out_start >= output.end {
            return result;
        }
        push(segment.clone(), out_start, &mut result);
        out_start += segment.len();
        prev_start = d.end;
    }
    if out_start < output.end {
        // Last kept segment runs to the end of the previous level.
        push(
            prev_start..prev_start + (output.end - out_start),
            out_start,
            &mut result,
        );
    }
    result
}

/// Sorts and merges row ranges, dropping empty ones.
pub fn normalize_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|r| !r.is_empty());
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}
