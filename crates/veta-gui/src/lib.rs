//! Veta's desktop interface, built with iced.
//!
//! The iced state ([`app`]) holds the core `Workbook` (the model) plus
//! GUI-only state (tabs, grid scroll positions, panes, settings). Messages
//! that change data are turned into core commands; see
//! `docs/ARCHITECTURE.md`.

mod app;
mod dialogs;
mod grid;
mod panes;
mod platform;
mod ribbon;
mod settings;

pub use app::run;
