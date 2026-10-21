//! Native open/save dialogs with consistent file types.

use std::path::{Path, PathBuf};

/// Kinds of files Veta reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Parquet,
    Csv,
    /// A JSON array of objects.
    Json,
    /// One JSON object per line.
    JsonLines,
    Recipe,
}

impl FileKind {
    pub fn label(self) -> String {
        veta_core::i18n::tr(match self {
            Self::Parquet => "ST_FILE_KIND_PARQUET",
            Self::Csv => "ST_FILE_KIND_CSV",
            Self::Json => "ST_FILE_KIND_JSON",
            Self::JsonLines => "ST_FILE_KIND_JSON_LINES",
            Self::Recipe => "ST_FILE_KIND_RECIPE",
        })
    }

    /// Extensions, the preferred one first.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Parquet => &["parquet", "parq", "pq"],
            Self::Csv => &["csv", "tsv", "txt"],
            Self::Json => &["json"],
            Self::JsonLines => &["ndjson", "jsonl"],
            Self::Recipe => &["json"],
        }
    }

    pub fn extension(self) -> &'static str {
        self.extensions()[0]
    }

    /// Filter name as shown by the dialog, e.g. `CSV file (*.csv, *.tsv)`.
    pub fn filter_name(self) -> String {
        let patterns: Vec<String> = self.extensions().iter().map(|e| format!("*.{e}")).collect();
        format!("{} ({})", self.label(), patterns.join(", "))
    }

    /// `path`, with this kind's extension added unless it already has one
    /// of them.
    pub fn ensure_extension(self, mut path: PathBuf) -> PathBuf {
        let has = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.extensions().iter().any(|x| x.eq_ignore_ascii_case(e)));
        if !has {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            path.set_file_name(format!("{name}.{}", self.extension()));
        }
        path
    }
}

/// `name` with its last extension replaced by `kind`'s.
pub fn suggested_name(name: &str, kind: FileKind) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    format!("{stem}.{}", kind.extension())
}

fn dialog(title: &str, kinds: &[FileKind], dir: Option<&Path>) -> rfd::AsyncFileDialog {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
    for kind in kinds {
        dialog = dialog.add_filter(kind.filter_name(), kind.extensions());
    }
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    dialog.add_filter(veta_core::i18n::tr("ST_FILE_KIND_ALL"), &["*"])
}

/// Asks where to save a file of `kind`.
pub async fn save(
    title: String,
    kind: FileKind,
    name: String,
    dir: Option<PathBuf>,
) -> Option<PathBuf> {
    let handle = dialog(&title, &[kind], dir.as_deref())
        .set_file_name(name)
        .save_file()
        .await?;
    Some(kind.ensure_extension(handle.path().to_path_buf()))
}

/// Asks for one file of the given kinds.
pub async fn open_one(title: String, kinds: Vec<FileKind>) -> Option<PathBuf> {
    let handle = dialog(&title, &kinds, None).pick_file().await?;
    Some(handle.path().to_path_buf())
}

/// Asks for any number of files of the given kinds.
pub async fn open_many(title: String, kinds: Vec<FileKind>) -> Option<Vec<PathBuf>> {
    let handles = dialog(&title, &kinds, None).pick_files().await?;
    Some(handles.iter().map(|h| h.path().to_path_buf()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_extensions() {
        assert_eq!(
            FileKind::Csv.filter_name(),
            "CSV file (*.csv, *.tsv, *.txt)"
        );
        assert_eq!(
            suggested_name("sales.csv", FileKind::Parquet),
            "sales.parquet"
        );
        assert_eq!(suggested_name("sales", FileKind::Recipe), "sales.json");
        assert_eq!(
            FileKind::Parquet.ensure_extension("a/b.PQ".into()),
            PathBuf::from("a/b.PQ")
        );
        assert_eq!(
            FileKind::Recipe.ensure_extension("a/sales.recipe".into()),
            PathBuf::from("a/sales.recipe.json")
        );
        assert_eq!(
            FileKind::JsonLines.ensure_extension("out".into()),
            PathBuf::from("out.ndjson")
        );
    }
}
