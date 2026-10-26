//! Row selections: which input rows a step keeps, in their order. Stored as
//! ranges, a list of rows or a mask (one bit per input row), whichever is
//! smallest.

use std::ops::{Range, RangeFull};

use arrow::array::{Array, BooleanArray};
use arrow::buffer::BooleanBuffer;

use crate::error::{Result, invalid};

/// Rows given to a method: all of them (`..`), a range, a list of row
/// numbers or a mask with one value per row (`true` picks the row).
#[derive(Debug, Clone)]
pub enum Rows {
    All,
    Range(Range<usize>),
    List(Vec<usize>),
    Mask(BooleanBuffer),
}

impl From<RangeFull> for Rows {
    fn from(_: RangeFull) -> Self {
        Rows::All
    }
}

impl From<Range<usize>> for Rows {
    fn from(range: Range<usize>) -> Self {
        Rows::Range(range)
    }
}

impl From<Vec<usize>> for Rows {
    fn from(rows: Vec<usize>) -> Self {
        Rows::List(rows)
    }
}

impl From<&[usize]> for Rows {
    fn from(rows: &[usize]) -> Self {
        Rows::List(rows.to_vec())
    }
}

impl<const N: usize> From<[usize; N]> for Rows {
    fn from(rows: [usize; N]) -> Self {
        Rows::List(rows.to_vec())
    }
}

impl From<Vec<bool>> for Rows {
    fn from(mask: Vec<bool>) -> Self {
        Rows::Mask(BooleanBuffer::from(mask))
    }
}

impl From<&[bool]> for Rows {
    fn from(mask: &[bool]) -> Self {
        Rows::Mask(BooleanBuffer::from(mask))
    }
}

/// Nulls don't pick their row.
impl From<&BooleanArray> for Rows {
    fn from(mask: &BooleanArray) -> Self {
        let values = match mask.nulls() {
            Some(nulls) => mask.values() & nulls.inner(),
            None => mask.values().clone(),
        };
        Rows::Mask(values)
    }
}

/// Bits per block of a mask whose kept rows are counted ahead, to find the
/// n-th kept row without counting from the start.
const BLOCK: usize = 64 * 1024;

/// The input rows a step keeps, in input order.
#[derive(Debug)]
pub(crate) enum Selection {
    /// Sorted, disjoint, non-empty ranges, with the kept rows before each.
    Ranges {
        ranges: Vec<Range<usize>>,
        before: Vec<usize>,
    },
    /// Sorted row numbers, each once.
    List(Vec<usize>),
    /// One bit per input row, with the kept rows before each block and in
    /// all.
    Mask {
        bits: BooleanBuffer,
        before: Vec<usize>,
        kept: usize,
    },
}

impl Selection {
    /// The rows `rows` picks out of `input_rows` rows, or all the others
    /// when `drop`.
    pub(crate) fn new(rows: Rows, input_rows: usize, drop: bool) -> Result<Self> {
        let bits = match rows {
            Rows::All => return Self::new(Rows::Range(0..input_rows), input_rows, drop),
            Rows::Range(range) => {
                if range.end > input_rows {
                    return Err(invalid!(RowPastEnd, row = range.end, rows = input_rows));
                }
                let range = range.start.min(range.end)..range.end;
                let ranges = match drop {
                    false => vec![range],
                    true => vec![0..range.start, range.end..input_rows],
                };
                return Ok(Self::from_ranges(ranges));
            }
            Rows::List(mut list) => {
                if let Some(&row) = list.iter().find(|&&r| r >= input_rows) {
                    return Err(invalid!(RowPastEnd, row = row, rows = input_rows));
                }
                list.sort_unstable();
                list.dedup();
                if !drop {
                    return Ok(Self::smallest_of_list(list, input_rows));
                }
                let mut bits = vec![true; input_rows];
                for row in list {
                    bits[row] = false;
                }
                return Ok(Self::from_mask(BooleanBuffer::from(bits)));
            }
            Rows::Mask(bits) => bits,
        };
        if bits.len() != input_rows {
            return Err(invalid!(
                MaskLength,
                found = bits.len(),
                expected = input_rows
            ));
        }
        Ok(Self::from_mask(match drop {
            true => !&bits,
            false => bits,
        }))
    }

    fn from_ranges(ranges: Vec<Range<usize>>) -> Self {
        let ranges: Vec<_> = ranges.into_iter().filter(|r| !r.is_empty()).collect();
        let mut before = Vec::with_capacity(ranges.len());
        let mut kept = 0;
        for range in &ranges {
            before.push(kept);
            kept += range.len();
        }
        Self::Ranges { ranges, before }
    }

    /// The smallest form of a mask: ranges (16 bytes each), a list (8 bytes
    /// a row) or the mask (1 bit a row).
    fn from_mask(bits: BooleanBuffer) -> Self {
        let kept = bits.count_set_bits();
        let ranges: Vec<Range<usize>> = bits.set_slices().map(|(a, b)| a..b).collect();
        let (as_ranges, as_list, as_mask) = (ranges.len() * 16, kept * 8, bits.len() / 8);
        if as_ranges <= as_list && as_ranges <= as_mask {
            return Self::from_ranges(ranges);
        }
        if as_list <= as_mask {
            return Self::List(bits.set_indices().collect());
        }
        Self::mask(bits)
    }

    fn smallest_of_list(list: Vec<usize>, input_rows: usize) -> Self {
        let runs = 1 + list.windows(2).filter(|w| w[1] != w[0] + 1).count();
        if list.is_empty() || runs * 16 <= list.len() * 8 {
            return Self::from_ranges(runs_of(&list));
        }
        if list.len() * 8 <= input_rows / 8 {
            return Self::List(list);
        }
        let mut bits = vec![false; input_rows];
        for row in list {
            bits[row] = true;
        }
        Self::mask(BooleanBuffer::from(bits))
    }

    fn mask(bits: BooleanBuffer) -> Self {
        let mut before = Vec::with_capacity(bits.len().div_ceil(BLOCK));
        let mut kept = 0;
        for start in (0..bits.len()).step_by(BLOCK) {
            before.push(kept);
            kept += bits
                .slice(start, BLOCK.min(bits.len() - start))
                .count_set_bits();
        }
        Self::Mask { bits, before, kept }
    }

    /// Kept rows.
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Ranges { ranges, before } => match (ranges.last(), before.last()) {
                (Some(last), Some(b)) => b + last.len(),
                _ => 0,
            },
            Self::List(list) => list.len(),
            Self::Mask { kept, .. } => *kept,
        }
    }

    /// The input rows of output rows `output`, as sorted runs.
    pub(crate) fn runs(&self, output: Range<usize>) -> Vec<Range<usize>> {
        let mut runs = Vec::new();
        if output.is_empty() {
            return runs;
        }
        match self {
            Self::Ranges { ranges, before } => {
                let first = before.partition_point(|&b| b <= output.start) - 1;
                let mut at = output.start;
                for (range, &b) in ranges[first..].iter().zip(&before[first..]) {
                    if at >= output.end {
                        break;
                    }
                    let start = range.start + (at - b);
                    let end = range.end.min(range.start + (output.end - b));
                    runs.push(start..end);
                    at = b + (end - range.start);
                }
            }
            Self::List(list) => runs = runs_of(&list[output]),
            Self::Mask { bits, before, .. } => {
                let block = before.partition_point(|&b| b <= output.start) - 1;
                let mut seen = before[block];
                let mut wanted = output.len();
                for (a, b) in bits
                    .slice(block * BLOCK, bits.len() - block * BLOCK)
                    .set_slices()
                {
                    let (a, b) = (a + block * BLOCK, b + block * BLOCK);
                    // Skip the kept rows before `output.start`.
                    let skip = output.start.saturating_sub(seen).min(b - a);
                    seen += b - a;
                    let start = a + skip;
                    let end = b.min(start + wanted);
                    if start < end {
                        runs.push(start..end);
                        wanted -= end - start;
                    }
                    if wanted == 0 {
                        break;
                    }
                }
            }
        }
        runs
    }
}

/// Sorted row numbers as runs of consecutive rows.
fn runs_of(rows: &[usize]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for &row in rows {
        match runs.last_mut() {
            Some(run) if run.end == row => run.end += 1,
            _ => runs.push(row..row + 1),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(selection: &Selection, output: Range<usize>) -> Vec<usize> {
        selection.runs(output).into_iter().flatten().collect()
    }

    #[test]
    fn every_form_gives_the_same_rows() {
        let n = 200_000;
        let pick = |i: usize| i.is_multiple_of(3) || (1000..1500).contains(&i);
        let wanted: Vec<usize> = (0..n).filter(|&i| pick(i)).collect();
        let mask: Vec<bool> = (0..n).map(pick).collect();
        let from_mask = Selection::new(mask.clone().into(), n, false).unwrap();
        let from_list = Selection::new(wanted.clone().into(), n, false).unwrap();
        assert!(matches!(from_mask, Selection::Mask { .. }));
        for selection in [&from_mask, &from_list] {
            assert_eq!(selection.len(), wanted.len());
            assert_eq!(kept(selection, 0..wanted.len()), wanted);
            assert_eq!(kept(selection, 60_000..60_010), wanted[60_000..60_010]);
        }
        let dropped = Selection::new(mask.into(), n, true).unwrap();
        let others: Vec<usize> = (0..n).filter(|&i| !pick(i)).collect();
        assert_eq!(kept(&dropped, 0..others.len()), others);
    }

    #[test]
    fn small_forms() {
        let ranges = Selection::new((10..20).into(), 100, true).unwrap();
        assert_eq!(ranges.len(), 90);
        assert_eq!(ranges.runs(5..15), vec![5..10, 20..25]);
        let list = Selection::new([7, 3, 3, 900_000].into(), 1_000_000, false).unwrap();
        assert!(matches!(list, Selection::List(_)));
        assert_eq!(kept(&list, 0..3), vec![3, 7, 900_000]);
        let ranges = Selection::from_mask(BooleanBuffer::from(vec![true; 1024]));
        assert!(matches!(ranges, Selection::Ranges { .. }));
        assert!(Selection::new((0..101).into(), 100, false).is_err());
        assert!(Selection::new([100].into(), 100, false).is_err());
        assert!(Selection::new(vec![true; 3].into(), 4, false).is_err());
    }
}
