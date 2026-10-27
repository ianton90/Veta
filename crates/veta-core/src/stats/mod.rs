//! Column statistics: the summary of each column (summary.rs), added up
//! piece by piece (accumulator.rs), with distinct counts (distinct.rs),
//! a sample for quantiles (sample.rs), the shape of the values
//! (distribution.rs), and everything in full on request (full.rs).
//! aggregate.rs: the statistics of the new core's `DataFrame`, each one
//! computing only what it needs.

mod accumulator;
mod aggregate;
mod distinct;
mod distribution;
mod full;
mod sample;
mod summary;

pub(crate) use aggregate::{Aggregate, Needs};
pub use aggregate::{Estimate, Interpolation, Stat, StatValue};
pub use distinct::COUNTED_LIMIT;
pub use distribution::{DENSITY_POINTS, HISTOGRAM_BINS};
pub use sample::SAMPLE_SIZE;
pub use summary::*;
