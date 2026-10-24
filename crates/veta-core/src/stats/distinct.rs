//! Counting distinct values: by 64-bit hash, exactly while there are few,
//! then estimated with HyperLogLog. Exact counts also say how often each
//! value is seen and its first row, for the most frequent values; once
//! estimated, a sketch keeps the frequent ones approximately.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use arrow::array::{Array, ArrayRef};
use arrow::datatypes::DataType;
use arrow::row::RowConverter;

use super::EXACT_DISTINCT_LIMIT;
use super::sample::Sample;
use crate::error::Result;

/// Adds the hashes of `array`'s valid values (rows from `first_row` on) to
/// `distinct`. Numbers are hashed from their value, text from its bytes,
/// other types through the row format.
pub(super) fn insert_hashes(
    array: &ArrayRef,
    first_row: u64,
    converter: &RowConverter,
    distinct: &mut Distinct,
) -> Result<()> {
    let row = |i: usize| first_row + i as u64;
    use arrow::array::AsArray;
    match array.data_type() {
        // Text straight from its offsets and bytes.
        DataType::Utf8 => {
            let values = array.as_string::<i32>();
            let (offsets, bytes) = (values.value_offsets(), values.value_data());
            each_valid(array, |i| {
                let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
                distinct.insert(hash_bytes(&bytes[start..end]), row(i));
            });
        }
        DataType::LargeUtf8 => {
            let values = array.as_string::<i64>();
            let (offsets, bytes) = (values.value_offsets(), values.value_data());
            each_valid(array, |i| {
                let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
                distinct.insert(hash_bytes(&bytes[start..end]), row(i));
            });
        }
        t if t.is_primitive() && t.primitive_width().is_some_and(|w| w <= 16) => {
            let width = t.primitive_width().unwrap_or(0);
            let data = array.to_data();
            let bytes = &data.buffers()[0].as_slice()[data.offset() * width..];
            // Little-endian words, zero-extended: the same value always
            // gives the same hash.
            let word = |b: &[u8]| u64::from_le_bytes(b.try_into().expect("8 bytes"));
            match width {
                8 => each_valid(array, |i| {
                    distinct.insert(mix(word(&bytes[i * 8..i * 8 + 8]) ^ SEED), row(i));
                }),
                16 => each_valid(array, |i| {
                    let (low, high) = (&bytes[i * 16..i * 16 + 8], &bytes[i * 16 + 8..i * 16 + 16]);
                    distinct.insert(mix(word(low) ^ mix(word(high) ^ SEED)), row(i));
                }),
                _ => each_valid(array, |i| {
                    distinct.insert(
                        mix(tail_word(&bytes[i * width..(i + 1) * width]) ^ SEED),
                        row(i),
                    );
                }),
            }
        }
        // At most two values: how many of each, and where each first is.
        DataType::Boolean => {
            let values = array.as_boolean();
            let trues = values.true_count();
            let falses = values.len() - values.null_count() - trues;
            for (value, n) in [(true, trues), (false, falses)] {
                let first =
                    (0..values.len()).find(|&i| values.is_valid(i) && values.value(i) == value);
                if let (true, Some(i)) = (n > 0, first) {
                    distinct.insert_n(mix(u64::from(value) ^ SEED), n as u64, row(i));
                }
            }
        }
        _ => {
            let rows = converter.convert_columns(std::slice::from_ref(array))?;
            each_valid(array, |i| {
                distinct.insert(hash_bytes(rows.row(i).data()), row(i))
            });
        }
    }
    Ok(())
}

/// Calls `f` with the index of each valid value of `array`.
fn each_valid(array: &ArrayRef, mut f: impl FnMut(usize)) {
    match array.nulls().filter(|n| n.null_count() > 0) {
        Some(nulls) => nulls.valid_indices().for_each(f),
        None => (0..array.len()).for_each(&mut f),
    }
}

/// Mixed into every hash, so no value hashes to 0.
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// A fast 64-bit hash of `bytes`, well mixed in every bit (HyperLogLog uses
/// the top bits).
fn hash_bytes(bytes: &[u8]) -> u64 {
    let step = |h: u64, word: u64| {
        let h = (h ^ word).wrapping_mul(0x9fb2_1c65_1e98_df25);
        h ^ (h >> 29)
    };
    let mut h = SEED ^ bytes.len() as u64;
    let mut words = bytes.chunks_exact(8);
    for word in &mut words {
        h = step(h, u64::from_le_bytes(word.try_into().expect("8 bytes")));
    }
    let rest = words.remainder();
    if !rest.is_empty() {
        h = step(h, tail_word(rest));
    }
    mix(h)
}

/// 1–7 bytes as a little-endian word, zero-extended, read in
/// fixed-size pieces (a copy of any length would call `memcpy`).
fn tail_word(mut rest: &[u8]) -> u64 {
    let mut word = 0;
    let mut shift = 0;
    if rest.len() >= 4 {
        word = u64::from(u32::from_le_bytes(rest[..4].try_into().expect("4 bytes")));
        shift = 32;
        rest = &rest[4..];
    }
    if rest.len() >= 2 {
        word |= u64::from(u16::from_le_bytes(rest[..2].try_into().expect("2 bytes"))) << shift;
        shift += 16;
        rest = &rest[2..];
    }
    if let [last] = rest {
        word |= u64::from(*last) << shift;
    }
    word
}

/// splitmix64's finalizer.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A hasher for values that are hashes already.
#[derive(Default)]
struct Identity(u64);

impl Hasher for Identity {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = hash_bytes(bytes);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

/// Distinct values whose counts are kept exactly (with a row each), for
/// the most frequent values; past this, only that each value was seen.
pub const COUNTED_LIMIT: usize = 65_536;

/// How often a value was seen, and the first row it was seen in (32 bits
/// each keep the map small; a row past `u32::MAX` is [`NO_ROW`], and the
/// value's row is then looked for in the row sample).
#[derive(Debug, Clone, Copy)]
struct Seen {
    count: u32,
    row: u32,
}

const NO_ROW: u32 = u32::MAX;

type Hashes<V> = HashMap<u64, V, BuildHasherDefault<Identity>>;

/// The `n` most seen of (hash, row, count), most seen first (ties: the
/// earlier row first).
fn most_seen(mut all: Vec<(u64, u64, u64)>, n: usize) -> Vec<(u64, u64, u64)> {
    let order = |a: &(u64, u64, u64), b: &(u64, u64, u64)| b.2.cmp(&a.2).then(a.1.cmp(&b.1));
    if all.len() > n && n > 0 {
        all.select_nth_unstable_by(n - 1, order);
        all.truncate(n);
    }
    all.sort_by(order);
    all
}

/// Distinct hashes: counted while few, a set after that, HyperLogLog when
/// too many. A sample of rows (their value's hash) gives the frequent
/// values once they are no longer counted.
#[derive(Default)]
pub(super) struct Distinct {
    /// Each value's count and first row, up to [`COUNTED_LIMIT`] values.
    counted: Hashes<Seen>,
    /// Past that, the values seen.
    exact: HashSet<u64, BuildHasherDefault<Identity>>,
    /// Whether `exact` replaced `counted`.
    uncounted: bool,
    estimate: Option<HyperLogLog>,
    /// (hash, row) of a uniform sample of rows.
    rows: Sample<(u64, u64)>,
}

impl Distinct {
    /// Adds the value with this hash in row `row`.
    pub(super) fn insert(&mut self, hash: u64, row: u64) {
        self.rows.add(row, (hash, row));
        self.count(hash, 1, row);
    }

    /// Adds `n` values with this hash, the first at `row` (left out of
    /// the row sample: only for values that stay counted, like booleans).
    pub(super) fn insert_n(&mut self, hash: u64, n: u64, row: u64) {
        self.count(hash, n, row);
    }

    fn count(&mut self, hash: u64, n: u64, row: u64) {
        if let Some(hll) = &mut self.estimate {
            hll.insert(hash);
        } else if self.uncounted {
            self.exact.insert(hash);
            if self.exact.len() > EXACT_DISTINCT_LIMIT {
                self.start_estimating();
            }
        } else {
            let row = u32::try_from(row).unwrap_or(NO_ROW);
            let seen = self.counted.entry(hash).or_insert(Seen { count: 0, row });
            seen.count = seen
                .count
                .saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
            seen.row = seen.row.min(row);
            if self.counted.len() > COUNTED_LIMIT {
                self.stop_counting();
            }
        }
    }

    /// Keeps only which values were seen.
    pub(super) fn stop_counting(&mut self) {
        if self.uncounted {
            return;
        }
        self.uncounted = true;
        self.exact
            .extend(self.counted.drain().map(|(hash, _)| hash));
        self.counted.shrink_to_fit();
    }

    /// Switches to the estimate, with the values seen so far.
    pub(super) fn start_estimating(&mut self) {
        if self.estimate.is_some() {
            return;
        }
        self.stop_counting();
        let mut hll = HyperLogLog::new();
        for hash in self.exact.drain() {
            hll.insert(hash);
        }
        self.exact.shrink_to_fit();
        self.estimate = Some(hll);
    }

    /// Adds the values counted by `other`.
    pub(super) fn merge(&mut self, other: Distinct) {
        self.rows.merge(other.rows);
        if let Some(theirs) = other.estimate {
            self.start_estimating();
            if let Some(mine) = &mut self.estimate {
                for (a, b) in mine.registers.iter_mut().zip(theirs.registers) {
                    *a = (*a).max(b);
                }
            }
            return;
        }
        if other.uncounted {
            self.stop_counting();
        }
        for hash in other.exact {
            self.count(hash, 1, 0);
        }
        for (hash, seen) in other.counted {
            self.count(hash, u64::from(seen.count), u64::from(seen.row));
        }
    }

    pub(super) fn count_distinct(&self) -> u64 {
        match &self.estimate {
            Some(hll) => hll.estimate(),
            None if self.uncounted => self.exact.len() as u64,
            None => self.counted.len() as u64,
        }
    }

    pub(super) fn estimated(&self) -> bool {
        self.estimate.is_some()
    }

    /// Whether the most frequent values come from the row sample (too
    /// many distinct values to count each), their counts estimates.
    pub(super) fn sampled_counts(&self) -> bool {
        self.uncounted
    }

    /// The `n` most frequent values as (a row holding it, count), most
    /// frequent first. Past [`COUNTED_LIMIT`] values, from the row sample:
    /// the values there more than once, their counts scaled up.
    pub(super) fn most_frequent(self, n: usize) -> Vec<(u64, u64)> {
        let (sampled, seen) = self.rows.values();
        // The sampled rows by hash: how many, and the earliest.
        let mut sample_rows: Hashes<(u64, u64)> = HashMap::default();
        for (hash, row) in &sampled {
            let entry = sample_rows.entry(*hash).or_insert((0, *row));
            entry.0 += 1;
            entry.1 = entry.1.min(*row);
        }
        let top = if self.uncounted {
            let scale = seen as f64 / sampled.len().max(1) as f64;
            let repeated = sample_rows
                .iter()
                .filter(|(_, (count, _))| *count > 1)
                .map(|(hash, (count, row))| (*hash, *row, (*count as f64 * scale).round() as u64))
                .collect();
            most_seen(repeated, n)
        } else {
            let all = self
                .counted
                .into_iter()
                .map(|(hash, s)| (hash, u64::from(s.row), u64::from(s.count)))
                .collect();
            most_seen(all, n)
        };
        top.into_iter()
            .filter_map(|(hash, row, count)| {
                let row = match row == u64::from(NO_ROW) {
                    true => sample_rows.get(&hash)?.1,
                    false => row,
                };
                Some((row, count))
            })
            .collect()
    }
}

/// HyperLogLog with 2^14 registers (about 0.8% standard error).
struct HyperLogLog {
    registers: Vec<u8>,
}

impl HyperLogLog {
    const BITS: u32 = 14;

    fn new() -> Self {
        Self {
            registers: vec![0; 1 << Self::BITS],
        }
    }

    fn insert(&mut self, hash: u64) {
        let index = (hash >> (64 - Self::BITS)) as usize;
        let rest = hash << Self::BITS;
        let rank = (rest.leading_zeros() + 1).min(64 - Self::BITS + 1) as u8;
        if rank > self.registers[index] {
            self.registers[index] = rank;
        }
    }

    fn estimate(&self) -> u64 {
        let m = self.registers.len() as f64;
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let sum: f64 = self.registers.iter().map(|&r| 2f64.powi(-(r as i32))).sum();
        let raw = alpha * m * m / sum;
        let zeros = self.registers.iter().filter(|&&r| r == 0).count();
        let estimate = if raw <= 2.5 * m && zeros > 0 {
            m * (m / zeros as f64).ln()
        } else {
            raw
        };
        estimate.round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::row::SortField;
    use std::sync::Arc;

    #[test]
    fn distinct_counts_every_type_and_slice() {
        use arrow::array::{BooleanArray, Int8Array, Int32Array, StringArray};
        let count = |array: ArrayRef| {
            let converter =
                RowConverter::new(vec![SortField::new(array.data_type().clone())]).unwrap();
            let mut distinct = Distinct::default();
            insert_hashes(&array, 0, &converter, &mut distinct).unwrap();
            distinct.count_distinct()
        };
        let bools = BooleanArray::from(vec![Some(true), None, Some(true), Some(false)]);
        assert_eq!(count(Arc::new(bools.clone())), 2);
        assert_eq!(count(Arc::new(bools.slice(0, 3))), 1, "nulls aren't values");
        assert_eq!(
            count(Arc::new(BooleanArray::from(vec![None::<bool>; 3]))),
            0
        );
        let small = Int8Array::from(vec![Some(1), Some(-1), None, Some(1), Some(0)]);
        assert_eq!(count(Arc::new(small.clone())), 3);
        assert_eq!(count(Arc::new(small.slice(2, 3))), 2);
        let ints = Int32Array::from_iter_values((0..1000).map(|i| i % 7));
        assert_eq!(count(Arc::new(ints.slice(3, 500))), 7);
        // Every length from empty to past two words, sliced.
        let words: Vec<String> = (0..20).map(|n| "x".repeat(n)).collect();
        let text = StringArray::from_iter_values(words.iter().chain(&words));
        assert_eq!(count(Arc::new(text.clone())), 20);
        assert_eq!(count(Arc::new(text.slice(5, 10))), 10);
        assert_eq!(tail_word(&[1, 2, 3]), 0x03_02_01);
        assert_eq!(tail_word(&[1, 2, 3, 4, 5, 6, 7]), 0x07_06_05_04_03_02_01);
    }

    #[test]
    fn most_frequent_exact_then_sketched() {
        let mut d = Distinct::default();
        for row in 0..1000u64 {
            d.insert(mix(row % 10), row);
        }
        // Ten values, a hundred each: the earliest first on ties.
        assert_eq!(d.most_frequent(3), [(0, 100), (1, 100), (2, 100)]);

        // Past the exact limit: a value in every tenth row stays on top,
        // its count close (a little low).
        let mut d = Distinct::default();
        let n = EXACT_DISTINCT_LIMIT as u64 * 2;
        let heavy = mix(u64::MAX);
        for row in 0..n {
            match row % 10 {
                3 => d.insert(heavy, row),
                _ => d.insert(mix(row), row),
            }
        }
        assert!(d.estimated());
        let top = d.most_frequent(1);
        let (row, count) = top[0];
        assert_eq!(row % 10, 3, "a row holding that value");
        let off = (count as f64 - n as f64 / 10.0).abs() / (n as f64 / 10.0);
        assert!(off < 0.1, "{count}");
    }

    #[test]
    fn hyperloglog_is_close() {
        let mut d = Distinct::default();
        let n = EXACT_DISTINCT_LIMIT as u64 * 2;
        for i in 0..n {
            d.insert(mix(i), i);
        }
        assert!(d.estimated());
        let error = (d.count_distinct() as f64 - n as f64).abs() / n as f64;
        assert!(error < 0.03, "error {error}");
    }
}
