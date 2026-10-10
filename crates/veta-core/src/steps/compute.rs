//! Full passes over a step's input, for steps that can't be evaluated one
//! window at a time (filter, sort, fill). The result is cached on the
//! pipeline so later reads are windowed again.

use std::ops::Range;
use std::sync::Arc;

use super::{Pipeline, Step, invalid};
use crate::error::{Error, Result};
use crate::source::DataSource;

/// Rows read per batch during a full pass.
pub(super) const PASS_ROWS: usize = 64 * 1024;

/// The cached result of a full pass.
#[derive(Debug)]
pub(crate) enum Cache {
    /// The input rows that survive, in order (filters).
    Rows(RowMap),
    /// The step's whole output (sort, fill).
    #[allow(dead_code)] // Used by sort and fill.
    Data(Arc<dyn DataSource>),
}

/// Kept input rows as sorted, disjoint ranges, with each range's position in
/// the output.
#[derive(Debug, Default)]
pub(crate) struct RowMap {
    ranges: Vec<Range<usize>>,
    /// Output row at which each range starts.
    starts: Vec<usize>,
    total: usize,
}

impl RowMap {
    /// Appends input rows `range`, which must come after all previous ones.
    pub(super) fn push(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let len = range.len();
        match self.ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => {
                self.starts.push(self.total);
                self.ranges.push(range);
            }
        }
        self.total += len;
    }

    /// Number of kept rows.
    pub(super) fn total(&self) -> usize {
        self.total
    }

    /// Input row ranges for output rows `output`.
    pub(super) fn map(&self, output: Range<usize>) -> Vec<Range<usize>> {
        let mut result = Vec::new();
        if output.is_empty() {
            return result;
        }
        let first = self
            .starts
            .partition_point(|&s| s <= output.start)
            .saturating_sub(1);
        for (range, &start) in self.ranges.iter().zip(&self.starts).skip(first) {
            if start >= output.end {
                break;
            }
            let from = output.start.max(start) - start;
            let to = output.end.min(start + range.len()) - start;
            if from < to {
                result.push(range.start + from..range.start + to);
            }
        }
        result
    }
}

/// A full pass to run, detached from the document so it can run on another
/// thread. Produce it with [`Pipeline::compute_job`].
#[derive(Debug, Clone)]
pub struct ComputeJob {
    pub(super) pipeline: Pipeline,
    pub(super) step: usize,
    /// Identities of the steps up to `step`, to check the result still
    /// applies when it is installed.
    pub(super) ids: Vec<u64>,
}

/// The result of a [`ComputeJob`], to give back to the pipeline.
#[derive(Debug)]
pub struct Computed {
    pub(super) step: usize,
    pub(super) ids: Vec<u64>,
    pub(super) cache: Arc<Cache>,
}

impl ComputeJob {
    /// Index of the step being computed.
    pub fn step(&self) -> usize {
        self.step
    }

    /// See [`Pipeline::compute_key`].
    pub fn key(&self) -> &[u64] {
        &self.ids
    }

    /// Runs the pass. `progress` gets the fraction done (0–1) and returns
    /// `false` to cancel, which makes this return [`Error::Cancelled`].
    pub fn run(self, progress: &mut dyn FnMut(f32) -> bool) -> Result<Computed> {
        let input_rows = self
            .pipeline
            .num_rows_at(self.step)
            .ok_or_else(|| invalid("the step's input is not evaluated".into()))?;
        let cache = match &self.pipeline.steps()[self.step] {
            Step::Filter(filter) => {
                let mut rows = RowMap::default();
                let mut start = 0;
                while start < input_rows {
                    let end = (start + PASS_ROWS).min(input_rows);
                    let batch = self.pipeline.read_at(self.step, start..end)?;
                    let mask = filter.evaluate(&batch)?;
                    for (from, to) in mask.values().set_slices() {
                        rows.push(start + from..start + to);
                    }
                    start = end;
                    if !progress(start as f32 / input_rows as f32) {
                        return Err(Error::Cancelled);
                    }
                }
                Cache::Rows(rows)
            }
            other => {
                return Err(invalid(format!(
                    "{} doesn't need computing",
                    other.describe()
                )));
            }
        };
        Ok(Computed {
            step: self.step,
            ids: self.ids,
            cache: Arc::new(cache),
        })
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn row_map() {
        let mut map = RowMap::default();
        map.push(2..4);
        map.push(4..5); // merges
        map.push(8..10);
        map.push(20..21);
        // input: [2 3 4] [8 9] [20] → output 0..6
        assert_eq!(map.total(), 6);
        assert_eq!(map.map(0..6), vec![2..5, 8..10, 20..21]);
        assert_eq!(map.map(1..4), vec![3..5, 8..9]);
        assert_eq!(map.map(5..6), vec![20..21]);
        assert!(map.map(3..3).is_empty());
    }
}
