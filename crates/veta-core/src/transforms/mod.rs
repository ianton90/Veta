//! What each transformation does to the data. The steps engine
//! ([`crate::steps`]) decides when they run; new transformations (scaling,
//! finding duplicates, …) go in their own files here.

pub mod cast;
pub(crate) mod columns;
pub mod fill;
pub mod filter;
pub mod nested;
pub mod sort;
pub mod split;
pub mod text;
