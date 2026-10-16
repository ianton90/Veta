//! Values to and from text: displaying values and types, parsing user
//! input, nested values as trees of nodes, and the clipboard format.

pub mod clipboard;
pub mod display;
pub mod node;
mod parse;

pub use parse::*;
