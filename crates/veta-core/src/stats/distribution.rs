//! The shape of a column's values, for drawing: a histogram and a smooth
//! density (for a violin plot), from the sorted sample of its values.

use serde::Serialize;

use super::sample::quantile;

/// Bars of the histogram.
pub const HISTOGRAM_BINS: usize = 20;

/// Points at which the density is given.
pub const DENSITY_POINTS: usize = 64;

/// Fine bins the density is smoothed from (a binned kernel estimate).
const FINE_BINS: usize = 256;

/// How a column's values (numbers, or the underlying numbers of dates and
/// times) are spread between `low` and `high`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Distribution {
    /// Smallest and largest sampled value.
    pub low: f64,
    pub high: f64,
    /// Values in each of [`HISTOGRAM_BINS`] equal bins from `low` to
    /// `high` (scaled to all values when sampled).
    pub bins: Vec<u64>,
    /// Smoothed density at [`DENSITY_POINTS`] evenly spaced points from
    /// `low` to `high`, the largest 1.
    pub density: Vec<f64>,
    /// First quartile, median and third quartile.
    pub quartiles: [f64; 3],
    /// From a sample of the values (there were more).
    pub estimated: bool,
}

/// The distribution of `sorted` values, a sample of `total`, over `range`
/// (the column's smallest and largest value; the sample's when `None`).
/// `None` with fewer than two distinct values.
pub(super) fn distribution(
    sorted: &[f64],
    total: u64,
    estimated: bool,
    range: Option<(f64, f64)>,
) -> Option<Distribution> {
    let (first, last) = (*sorted.first()?, *sorted.last()?);
    let (low, high) = range.unwrap_or((first, last));
    let width = high - low;
    if !(width > 0.0) || !width.is_finite() {
        return None;
    }
    let bin =
        |x: f64, bins: usize| (((x - low).max(0.0) / width * bins as f64) as usize).min(bins - 1);
    let scale = total as f64 / sorted.len() as f64;
    let mut bins = vec![0u64; HISTOGRAM_BINS];
    let mut fine = vec![0f64; FINE_BINS];
    for &x in sorted {
        bins[bin(x, HISTOGRAM_BINS)] += 1;
        fine[bin(x, FINE_BINS)] += 1.0;
    }
    let bins = bins
        .into_iter()
        .map(|n| (n as f64 * scale).round() as u64)
        .collect();
    let quartiles = [
        quantile(sorted, 0.25)?,
        quantile(sorted, 0.5)?,
        quantile(sorted, 0.75)?,
    ];
    let density = smooth(sorted, &fine, low, width, quartiles);
    Some(Distribution {
        low,
        high,
        bins,
        density,
        quartiles,
        estimated,
    })
}

/// A Gaussian kernel density from the fine bins, bandwidth by Silverman's
/// rule, scaled so its peak is 1.
fn smooth(sorted: &[f64], fine: &[f64], low: f64, width: f64, quartiles: [f64; 3]) -> Vec<f64> {
    let n = sorted.len() as f64;
    let mean = sorted.iter().sum::<f64>() / n;
    let sd = (sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
    let spread = match (quartiles[2] - quartiles[0]) / 1.34 {
        iqr if iqr > 0.0 => sd.min(iqr),
        _ => sd,
    };
    // At least a fine bin wide, so a few values still give a shape.
    let bandwidth = (0.9 * spread * n.powf(-0.2)).max(width / FINE_BINS as f64);
    let step = width / FINE_BINS as f64;
    let mut density: Vec<f64> = (0..DENSITY_POINTS)
        .map(|p| {
            let x = low + width * p as f64 / (DENSITY_POINTS - 1) as f64;
            fine.iter()
                .enumerate()
                .filter(|(_, count)| **count > 0.0)
                .map(|(i, count)| {
                    let center = low + step * (i as f64 + 0.5);
                    let z = (x - center) / bandwidth;
                    count * (-0.5 * z * z).exp()
                })
                .sum()
        })
        .collect();
    let peak = density.iter().cloned().fold(0.0, f64::max);
    if peak > 0.0 {
        density.iter_mut().for_each(|d| *d /= peak);
    }
    density
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_and_density() {
        let sorted: Vec<f64> = (0..1000).map(|i| (i / 10) as f64).collect();
        let d = distribution(&sorted, 2000, true, None).unwrap();
        assert_eq!((d.low, d.high), (0.0, 99.0));
        assert_eq!(d.bins.len(), HISTOGRAM_BINS);
        assert_eq!(d.bins.iter().sum::<u64>(), 2000, "scaled to every value");
        assert_eq!(d.density.len(), DENSITY_POINTS);
        assert!(d.density.iter().all(|x| (0.0..=1.0).contains(x)));
        assert!(d.density[DENSITY_POINTS / 2] > 0.8, "flat in the middle");
        assert_eq!(d.quartiles[1], 49.5);
        assert!(
            distribution(&[3.0, 3.0], 2, false, None).is_none(),
            "one value"
        );
        assert!(distribution(&[], 0, false, None).is_none());
        let wider = distribution(&sorted, 1000, false, Some((-99.0, 99.0))).unwrap();
        assert_eq!(wider.bins[..HISTOGRAM_BINS / 2].iter().sum::<u64>(), 0);
    }
}
