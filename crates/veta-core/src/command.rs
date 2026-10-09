//! Commands: every change to a [`Document`](crate::model::Document) is
//! expressed as one of these and applied by the
//! [`controller`](crate::controller). The GUI and the CLI build the same
//! commands.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Sets a key/value metadata entry, replacing the value if the key exists.
    SetMetadata { key: String, value: Option<String> },
    /// Removes a key/value metadata entry.
    RemoveMetadata { key: String },
}
