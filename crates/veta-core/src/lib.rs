//! Veta core: model, controller and storage. Has no UI dependencies; the GUI
//! and the CLI are thin layers on top of it. See `docs/ARCHITECTURE.md`.

pub mod controller;
pub mod error;
pub mod files;
pub mod formulas;
pub mod frame;
pub mod memory;
pub mod model;
mod ops;
mod parallel;
mod plan;
pub mod profile;
mod sources;
pub mod stats;
pub mod steps;
pub mod transforms;
pub mod values;

// Short paths for the modules the GUI and CLI use most.
pub use controller::{command, rows};
pub use files::parquet as io;
pub use files::{convert, recipe, source};
pub use model::history;
pub use values::{clipboard, display};

/// Version information. See `docs/RELEASING.md`.
pub mod version {
    /// The release version (`MAJOR.MINOR.PATCH`).
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    /// Build number: set by the release build, otherwise the commit count.
    /// Empty when built without git.
    pub const BUILD: &str = env!("VETA_BUILD");
    /// Short commit hash, empty when built without git.
    pub const COMMIT: &str = env!("VETA_COMMIT");
    /// E.g. `0.7.0 (build 412, a1b2c3d)`.
    pub const LONG: &str = env!("VETA_VERSION_LONG");
}

/// The Arrow crate used by core, re-exported so front ends use the same version.
pub use arrow;
pub use command::Command;
pub use display::PathPart;
pub use error::{Error, Result};
pub use frame::{
    Column, ColumnRef, DataFrame, Fill, IntoValue, Null, ReadOptions, ReplaceOptions, read_csv,
    read_csv_with, read_json, read_parquet, read_parquet_with,
};
pub use io::{OpenOptions, SaveJob, SaveResult};
pub use model::{
    ColumnSettings, Compression, Encoding, FormatVersion, StatisticsLevel, WriterSettings,
};
pub use model::{Document, DocumentId, Workbook};
pub use ops::Columns;
pub use plan::{Progress, Rows, Task};
pub use source::SourceMode;
pub use transforms::cast::{CastOptions, OnError};
pub use transforms::split::SplitBy;
pub use transforms::text::TextOp;
