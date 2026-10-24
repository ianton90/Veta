//! Column statistics: the summary of each column (summary.rs), added up
//! piece by piece (accumulator.rs), with distinct counts (distinct.rs)
//! a sample for quantiles (sample.rs) and the shape of the values
//! (distribution.rs).

mod accumulator;
mod distinct;
mod distribution;
mod sample;
mod summary;

pub use distribution::{DENSITY_POINTS, HISTOGRAM_BINS};
pub use sample::SAMPLE_SIZE;
pub use summary::*;
