use std::path::{Path, PathBuf};

use super::{FileMetadata, WriterSettings};

/// One open file (or a new, unsaved one).
///
/// Data source, steps and history are added by later milestones
/// (see `docs/ARCHITECTURE.md`).
#[derive(Debug, Default)]
pub struct Document {
    pub(crate) path: Option<PathBuf>,
    pub(crate) metadata: FileMetadata,
    pub(crate) writer: WriterSettings,
}

impl Document {
    /// A new, empty document that has never been saved.
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the document is saved, if it has been.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Name to show in tabs and window titles.
    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    pub fn metadata(&self) -> &FileMetadata {
        &self.metadata
    }

    pub fn writer_settings(&self) -> &WriterSettings {
        &self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_uses_file_name_or_untitled() {
        let mut doc = Document::new();
        assert_eq!(doc.title(), "Untitled");

        doc.path = Some(PathBuf::from("/data/sales.parquet"));
        assert_eq!(doc.title(), "sales.parquet");
    }
}
