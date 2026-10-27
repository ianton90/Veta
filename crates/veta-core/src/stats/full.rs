//! Statistics computed in full ([`StatsOptions::exact`](super::StatsOptions)):
//! one column at a time, every value is hashed (distinct values and, when
//! asked for, how often each is there) and every number kept for the
//! quartiles. One column at a time keeps memory to what that column needs.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::{Array, AsArray};
use arrow::datatypes::{DataType, Float64Type};
use arrow::row::{RowConverter, SortField};

use super::accumulator::as_numbers;
use super::distinct::{Identity, for_each_hash};
use crate::error::{Result, invalid};
use crate::steps::Pipeline;

/// What a full pass over one column found.
#[derive(Debug, Default)]
pub(super) struct Full {
    /// Distinct values (by 64-bit hash).
    pub(super) distinct: u64,
    /// The most frequent values as (a row holding it, count), when asked
    /// for.
    pub(super) most_frequent: Vec<(u64, u64)>,
    /// Every finite value as a float (numbers and times), in no order.
    pub(super) values: Vec<f64>,
}

/// A value's count and the first row it is in.
type Counts = HashMap<u64, (u64, u64), BuildHasherDefault<Identity>>;

/// What one piece of the column found.
enum Seen {
    Hashes(HashSet<u64, BuildHasherDefault<Identity>>),
    Counts(Counts),
}

/// Parts the hashes of the pieces are split into, by their top bits: a
/// value can be in several pieces, and appending the pieces' hashes to
/// parts, then sorting each part on its own on all cores, counts them
/// faster than putting them into one set.
pub(super) const PARTS: usize = 64;

/// Adds `hashes` to their parts.
pub(super) fn into_parts(parts: &mut [Vec<u64>], hashes: impl IntoIterator<Item = u64>) {
    for hash in hashes {
        parts[(hash >> (64 - PARTS.trailing_zeros())) as usize].push(hash);
    }
}

/// Distinct hashes in `parts`, each part sorted on its own, on all cores.
pub(super) fn count_parts(mut parts: Vec<Vec<u64>>) -> u64 {
    let chunk = PARTS.div_ceil(crate::parallel::threads().max(1));
    let total = AtomicU64::new(0);
    std::thread::scope(|scope| {
        for group in parts.chunks_mut(chunk) {
            let total = &total;
            scope.spawn(move || {
                for part in group {
                    part.sort_unstable();
                    part.dedup();
                    total.fetch_add(part.len() as u64, Ordering::Relaxed);
                }
            });
        }
    });
    total.into_inner()
}

/// Reads column `column` of `pipeline`'s output (`rows` rows, after
/// `level` steps) in pieces on all cores. Values are hashed when `hashed`
/// gives their type (`None`: not hashed); `top` most frequent values are
/// kept (0: none); `numbers` keeps the values for quartiles.
#[allow(clippy::too_many_arguments)]
pub(super) fn full_column(
    pipeline: &Pipeline,
    level: usize,
    rows: Range<usize>,
    column: &str,
    hashed: Option<DataType>,
    top: usize,
    numbers: bool,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Full> {
    let name = column.to_owned();
    let total = rows.len().max(1);
    let converter = match hashed {
        Some(data_type) => Some(RowConverter::new(vec![SortField::new(data_type)])?),
        None => None,
    };
    let hashing = converter.is_some();
    let pieces = pipeline.read_parallel(
        level,
        rows,
        Some(vec![name.clone()]),
        move |batches, first| {
            let mut seen = match top > 0 {
                true => Seen::Counts(Counts::default()),
                false => Seen::Hashes(HashSet::default()),
            };
            let mut values = Vec::new();
            let mut row = first as u64;
            for batch in &batches {
                let array = batch
                    .column_by_name(&name)
                    .ok_or_else(|| invalid!(StatsFailed))?;
                if let Some(converter) = &converter {
                    for_each_hash(array, row, converter, |hash, n, at| match &mut seen {
                        Seen::Hashes(set) => {
                            set.insert(hash);
                        }
                        Seen::Counts(counts) => {
                            let entry = counts.entry(hash).or_insert((0, at));
                            entry.0 += n;
                            entry.1 = entry.1.min(at);
                        }
                    })?;
                }
                if numbers && let Some(floats) = as_numbers(array)? {
                    let floats = floats.as_primitive::<Float64Type>();
                    values.extend(floats.iter().flatten().filter(|x| x.is_finite()));
                }
                row += array.len() as u64;
            }
            Ok((seen, values, row as usize - first))
        },
    );
    let mut parts = vec![Vec::new(); PARTS];
    let mut counts = Counts::default();
    let mut values = Vec::new();
    let mut done = 0;
    for piece in pieces {
        let (seen, piece_values, n) = piece?;
        match seen {
            Seen::Hashes(set) => into_parts(&mut parts, set),
            Seen::Counts(piece_counts) => {
                for (hash, (n, at)) in piece_counts {
                    let entry = counts.entry(hash).or_insert((0, at));
                    entry.0 += n;
                    entry.1 = entry.1.min(at);
                }
            }
        }
        values.extend(piece_values);
        done += n;
        if !progress(done as f32 / total as f32) {
            return Err(crate::error::Error::Cancelled);
        }
    }
    let distinct = match (hashing, top > 0) {
        (false, _) => 0,
        (true, true) => counts.len() as u64,
        (true, false) => count_parts(parts),
    };
    let mut most_frequent: Vec<(u64, u64)> =
        counts.into_values().map(|(n, row)| (row, n)).collect();
    let order = |a: &(u64, u64), b: &(u64, u64)| b.1.cmp(&a.1).then(a.0.cmp(&b.0));
    if most_frequent.len() > top && top > 0 {
        most_frequent.select_nth_unstable_by(top - 1, order);
        most_frequent.truncate(top);
    }
    most_frequent.sort_by(order);
    Ok(Full {
        distinct,
        most_frequent,
        values,
    })
}
