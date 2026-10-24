//! A uniform sample of a column's values, for quantiles (and the shapes
//! drawn from them): each row gets a key from its row number (Fibonacci
//! hashing: one multiply, and the picked rows spread evenly), and the
//! values of the [`SAMPLE_SIZE`] rows with the smallest keys are kept. The
//! same rows are picked however the pass is split into pieces, so pieces
//! merge exactly and results don't change from run to run. Up to
//! `SAMPLE_SIZE` values the sample is every value, and quantiles are exact.

/// Values kept per column.
pub const SAMPLE_SIZE: usize = 16_384;

/// 2^64 / φ: multiplying row numbers by it spreads them over the keys.
const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

/// The sampled values of one column (underlying numbers, for times; or
/// anything else kept per row).
#[derive(Debug, Clone)]
pub(super) struct Sample<T = f64> {
    /// (key, value); up to twice the sample size between prunes.
    kept: Vec<(u64, T)>,
    /// Keys above this are left out: the largest key kept at the last
    /// prune (`u64::MAX` until the sample was full).
    limit: u64,
    /// Values offered, sampled or not.
    seen: u64,
}

impl<T> Default for Sample<T> {
    fn default() -> Self {
        Self {
            kept: Vec::new(),
            limit: u64::MAX,
            seen: 0,
        }
    }
}

impl<T> Sample<T> {
    /// Offers the value of row `row`.
    pub(super) fn add(&mut self, row: u64, value: T) {
        self.seen += 1;
        let key = row.wrapping_mul(GOLDEN);
        if key >= self.limit {
            return;
        }
        self.kept.push((key, value));
        if self.kept.len() >= 2 * SAMPLE_SIZE {
            self.prune();
        }
    }

    /// Keeps only the sample's rows.
    fn prune(&mut self) {
        if self.kept.len() <= SAMPLE_SIZE {
            return;
        }
        self.kept
            .select_nth_unstable_by_key(SAMPLE_SIZE - 1, |(key, _)| *key);
        self.kept.truncate(SAMPLE_SIZE);
        self.limit = self
            .kept
            .iter()
            .map(|(key, _)| *key)
            .max()
            .unwrap_or(u64::MAX);
    }

    /// Adds the rows `other` sampled (other rows of the same column).
    pub(super) fn merge(&mut self, other: Sample<T>) {
        self.seen += other.seen;
        let limit = self.limit.min(other.limit);
        self.kept.extend(other.kept);
        if limit < u64::MAX {
            self.kept.retain(|(key, _)| *key <= limit);
        }
        self.limit = limit;
        self.prune();
    }

    /// The sampled values, and how many values were offered.
    pub(super) fn values(mut self) -> (Vec<T>, u64) {
        self.prune();
        (self.kept.into_iter().map(|(_, v)| v).collect(), self.seen)
    }
}

impl Sample<f64> {
    /// The sampled values, sorted, and how many values were offered.
    pub(super) fn finish(self) -> (Vec<f64>, u64) {
        let (mut values, seen) = self.values();
        values.sort_by(f64::total_cmp);
        (values, seen)
    }
}

/// The `p` quantile (0–1) of `sorted` values, interpolating between the
/// two nearest.
pub(super) fn quantile(sorted: &[f64], p: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let at = p.clamp(0.0, 1.0) * last as f64;
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    let weight = at - low as f64;
    Some(sorted[low] * (1.0 - weight) + sorted[high] * weight)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_samples_are_exact_and_pieces_merge() {
        let mut whole = Sample::default();
        for i in 0..1000u64 {
            whole.add(i, i as f64);
        }
        let (values, seen) = whole.finish();
        assert_eq!(seen, values.len() as u64);
        assert_eq!(quantile(&values, 0.5), Some(499.5));
        assert_eq!(quantile(&values, 0.0), Some(0.0));
        assert_eq!(quantile(&[], 0.5), None);

        // A large column split into pieces gives the same sample as one
        // pass, close to the true quantiles.
        let n = 200_000u64;
        let mut one = Sample::default();
        let mut pieces = vec![Sample::default(); 4];
        for i in 0..n {
            one.add(i, i as f64);
            pieces[(i * 4 / n) as usize].add(i, i as f64);
        }
        let mut merged = Sample::default();
        for piece in pieces {
            merged.merge(piece);
        }
        let (a, seen) = one.finish();
        let (b, _) = merged.finish();
        assert_eq!(seen, n);
        assert_eq!(a.len(), SAMPLE_SIZE);
        assert_eq!(a, b);
        let median = quantile(&a, 0.5).unwrap();
        assert!(
            (median - n as f64 / 2.0).abs() < n as f64 * 0.02,
            "{median}"
        );
    }
}
