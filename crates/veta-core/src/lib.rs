//! Veta core: model, controller and storage. Has no UI dependencies; the GUI
//! and the CLI are thin layers on top of it. See `docs/ARCHITECTURE.md`.

pub mod command;
pub mod controller;
pub mod convert;
pub mod display;
pub mod error;
pub mod history;
pub mod io;
pub mod model;
pub mod recipe;
pub mod source;
pub mod stats;
pub mod steps;
pub mod values;

/// Version information. See `docs/RELEASING.md`.
pub mod version {
    /// The release version (`MAJOR.MINOR.PATCH`).
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    /// Build number: set by the release build, otherwise the commit count.
    /// Empty when built without git.
    pub const BUILD: &str = env!("VETA_BUILD");
    /// Short commit hash, empty when built without git.
    pub const COMMIT: &str = env!("VETA_COMMIT");
    /// E.g. `0.1.0 (build 412, a1b2c3d)`.
    pub const LONG: &str = env!("VETA_VERSION_LONG");
}

/// The Arrow crate used by core, re-exported so front ends use the same version.
pub use arrow;
pub use command::Command;
pub use error::{Error, Result};
pub use io::{OpenOptions, SaveJob, SaveResult};
pub use model::{
    ColumnSettings, Compression, Encoding, FormatVersion, StatisticsLevel, WriterSettings,
};
pub use model::{Document, DocumentId, Workbook};
pub use source::SourceMode;
