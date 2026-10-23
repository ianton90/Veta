//! Column statistics: the summary of each column (summary.rs), added up
//! piece by piece (accumulator.rs), with distinct counts (distinct.rs).

mod accumulator;
mod distinct;
mod summary;

pub use summary::*;
