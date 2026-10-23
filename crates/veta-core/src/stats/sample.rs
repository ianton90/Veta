//! A uniform sample of a column's values, for quantiles (and the shapes
//! drawn from them): each row gets a key by hashing its row number, and the
//! values of the [`SAMPLE_SIZE`] rows with the smallest keys are kept. The
//! same rows are picked however the pass is split into pieces, so pieces
//! merge exactly and results don't change from run to run. Up to
//! `SAMPLE_SIZE` values the sample is every value, and quantiles are exact.

use super::distinct::mix;

/// Values kept per column.
pub const SAMPLE_SIZE: usize = 16_384;

/// Mixed into row numbers, so the sample doesn't follow the distinct
/// hashes.
const ROW_SEED: u64 = 0x5851_f42d_4c95_7f2d;

/// The sampled values of one column (underlying numbers, for times).
#[derive(Debug, Clone, Default)]
pub(super) struct Sample {
    /// (key, value); up to twice the sample size between prunes.
    kept: Vec<(u64, f64)>,
    /// Keys at or above this are left out: the largest key kept at the
    /// last prune (once the sample was full).
    limit: Option<u64>,
    /// Values offered, sampled or not.
    seen: u64,
}

impl Sample {
    /// Offers the value of row `row`.
    pub(super) fn add(&mut self, row: u64, value: f64) {
        self.seen += 1;
        let key = mix(row ^ ROW_SEED);
        if self.limit.is_some_and(|limit| key >= limit) {
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
        self.limit = self.kept.iter().map(|(key, _)| *key).max();
    }

    /// Adds the rows `other` sampled (other rows of the same column).
    pub(super) fn merge(&mut self, other: Sample) {
        self.seen += other.seen;
        let limit = match (self.limit, other.limit) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.kept.extend(other.kept);
        if let Some(limit) = limit {
            self.kept.retain(|(key, _)| *key <= limit);
        }
        self.limit = limit;
        self.prune();
    }

    /// The sampled values, sorted, and whether they are only a sample of
    /// the values (not all of them).
    pub(super) fn finish(mut self) -> (Vec<f64>, bool) {
        self.prune();
        let mut values: Vec<f64> = self.kept.into_iter().map(|(_, v)| v).collect();
        values.sort_by(f64::total_cmp);
        let sampled = self.seen > values.len() as u64;
        (values, sampled)
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
        let (values, sampled) = whole.finish();
        assert!(!sampled);
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
        let (a, sampled) = one.finish();
        let (b, _) = merged.finish();
        assert!(sampled);
        assert_eq!(a.len(), SAMPLE_SIZE);
        assert_eq!(a, b);
        let median = quantile(&a, 0.5).unwrap();
        assert!(
            (median - n as f64 / 2.0).abs() < n as f64 * 0.02,
            "{median}"
        );
    }
}
