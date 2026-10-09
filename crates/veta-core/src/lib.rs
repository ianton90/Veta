//! Veta core: model, controller and storage. Has no UI dependencies; the GUI
//! and the CLI are thin layers on top of it. See `docs/ARCHITECTURE.md`.

pub mod command;
pub mod controller;
pub mod display;
pub mod error;
pub mod history;
pub mod io;
pub mod model;
pub mod source;
pub mod steps;

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
