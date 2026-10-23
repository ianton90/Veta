//! Counting distinct values: by 64-bit hash, exactly while there are few,
//! then estimated with HyperLogLog.

use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

use arrow::array::{Array, ArrayRef};
use arrow::datatypes::DataType;
use arrow::row::RowConverter;

use super::EXACT_DISTINCT_LIMIT;
use crate::error::Result;

/// Adds the hashes of `array`'s valid values to `distinct`. Numbers are
/// hashed from their value, text from its bytes, other types through the
/// row format.
pub(super) fn insert_hashes(
    array: &ArrayRef,
    converter: &RowConverter,
    distinct: &mut Distinct,
) -> Result<()> {
    use arrow::array::AsArray;
    match array.data_type() {
        // Text straight from its offsets and bytes.
        DataType::Utf8 => {
            let values = array.as_string::<i32>();
            let (offsets, bytes) = (values.value_offsets(), values.value_data());
            each_valid(array, |i| {
                let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
                distinct.insert(hash_bytes(&bytes[start..end]));
            });
        }
        DataType::LargeUtf8 => {
            let values = array.as_string::<i64>();
            let (offsets, bytes) = (values.value_offsets(), values.value_data());
            each_valid(array, |i| {
                let (start, end) = (offsets[i] as usize, offsets[i + 1] as usize);
                distinct.insert(hash_bytes(&bytes[start..end]));
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
                    distinct.insert(mix(word(&bytes[i * 8..i * 8 + 8]) ^ SEED));
                }),
                16 => each_valid(array, |i| {
                    let (low, high) = (&bytes[i * 16..i * 16 + 8], &bytes[i * 16 + 8..i * 16 + 16]);
                    distinct.insert(mix(word(low) ^ mix(word(high) ^ SEED)));
                }),
                _ => each_valid(array, |i| {
                    distinct.insert(mix(tail_word(&bytes[i * width..(i + 1) * width]) ^ SEED));
                }),
            }
        }
        // At most two values: whether each is there.
        DataType::Boolean => {
            let values = array.as_boolean();
            let trues = values.true_count();
            let valid = values.len() - values.null_count();
            for (value, present) in [(true, trues > 0), (false, trues < valid)] {
                if present {
                    distinct.insert(mix(u64::from(value) ^ SEED));
                }
            }
        }
        _ => {
            let rows = converter.convert_columns(std::slice::from_ref(array))?;
            each_valid(array, |i| distinct.insert(hash_bytes(rows.row(i).data())));
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
pub(super) fn mix(mut z: u64) -> u64 {
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

/// Distinct hashes: a set while small, HyperLogLog after.
#[derive(Default)]
pub(super) struct Distinct {
    exact: HashSet<u64, BuildHasherDefault<Identity>>,
    estimate: Option<HyperLogLog>,
}

impl Distinct {
    pub(super) fn insert(&mut self, hash: u64) {
        match &mut self.estimate {
            Some(hll) => hll.insert(hash),
            None => {
                self.exact.insert(hash);
                if self.exact.len() > EXACT_DISTINCT_LIMIT {
                    self.start_estimating();
                }
            }
        }
    }

    /// Switches to the estimate, with the values seen so far.
    pub(super) fn start_estimating(&mut self) {
        if self.estimate.is_some() {
            return;
        }
        let mut hll = HyperLogLog::new();
        for h in self.exact.drain() {
            hll.insert(h);
        }
        self.exact.shrink_to_fit();
        self.estimate = Some(hll);
    }

    /// Adds the values counted by `other`.
    pub(super) fn merge(&mut self, other: Distinct) {
        match other.estimate {
            Some(theirs) => {
                self.start_estimating();
                if let Some(mine) = &mut self.estimate {
                    for (a, b) in mine.registers.iter_mut().zip(theirs.registers) {
                        *a = (*a).max(b);
                    }
                }
            }
            None => {
                for hash in other.exact {
                    self.insert(hash);
                }
            }
        }
    }

    pub(super) fn count(&self) -> u64 {
        match &self.estimate {
            Some(hll) => hll.estimate(),
            None => self.exact.len() as u64,
        }
    }

    pub(super) fn estimated(&self) -> bool {
        self.estimate.is_some()
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
            insert_hashes(&array, &converter, &mut distinct).unwrap();
            distinct.count()
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
    fn hyperloglog_is_close() {
        let mut d = Distinct::default();
        let n = EXACT_DISTINCT_LIMIT as u64 * 2;
        for i in 0..n {
            d.insert(mix(i));
        }
        assert!(d.estimated());
        let error = (d.count() as f64 - n as f64).abs() / n as f64;
        assert!(error < 0.03, "error {error}");
    }
}
