//! Column statistics: the summary of each column (summary.rs), added up
//! piece by piece (accumulator.rs), with distinct counts (distinct.rs),
//! a sample for quantiles (sample.rs), the shape of the values
//! (distribution.rs), and everything in full on request (full.rs).

mod accumulator;
mod distinct;
mod distribution;
mod full;
mod sample;
mod summary;

pub use distinct::COUNTED_LIMIT;
pub use distribution::{DENSITY_POINTS, HISTOGRAM_BINS};
pub use sample::SAMPLE_SIZE;
pub use summary::*;
