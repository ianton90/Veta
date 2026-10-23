//! Column statistics: the summary of each column (summary.rs), added up
//! piece by piece (accumulator.rs), with distinct counts (distinct.rs)
//! and a sample for quantiles (sample.rs).

mod accumulator;
mod distinct;
mod sample;
mod summary;

pub use sample::SAMPLE_SIZE;
pub use summary::*;
