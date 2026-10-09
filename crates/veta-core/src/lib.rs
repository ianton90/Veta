//! Veta core: model, controller and storage. Has no UI dependencies; the GUI
//! and the CLI are thin layers on top of it. See `docs/ARCHITECTURE.md`.

pub mod command;
pub mod controller;
pub mod error;
pub mod model;

pub use command::Command;
pub use error::{Error, Result};
pub use model::{Document, DocumentId, Workbook};
