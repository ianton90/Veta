//! Writing a file safely in place: into a new temporary file next to it,
//! renamed over it once complete, so a failed write never leaves a
//! half-written file (#162).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;

/// A temporary file next to a target, renamed over it by
/// [`Replacement::commit`] and deleted if dropped before.
#[derive(Debug)]
pub(crate) struct Replacement {
    temp: PathBuf,
    target: PathBuf,
    /// Permissions a new file gets here (what the system gives new files).
    fresh: std::fs::Permissions,
    committed: bool,
}

impl Replacement {
    /// Creates the temporary file for `target`. It is always a new file
    /// (never one already there, or a link someone left at its name), and
    /// only its owner can read it while it is written. A `target` that is a
    /// link is replaced where it points.
    pub(crate) fn create(target: &Path) -> Result<(Self, File)> {
        let target = match std::fs::symlink_metadata(target) {
            Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(target)?,
            _ => target.to_path_buf(),
        };
        let dir = match target.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut attempt = 0;
        loop {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let temp = dir.join(format!(
                ".{name}.veta-{}-{n}-{unique:08x}.tmp",
                std::process::id()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
            {
                Ok(file) => {
                    // The system's permissions for a new file, kept for a new
                    // target; owner-only while writing.
                    let fresh = file.metadata()?.permissions();
                    owner_only(&file)?;
                    let replacement = Self {
                        temp,
                        target,
                        fresh,
                        committed: false,
                    };
                    return Ok((replacement, file));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => {
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Gives the written file the target's permissions (those of a new file
    /// if there is no target yet) and renames it over the target.
    pub(crate) fn commit(mut self) -> Result<()> {
        let permissions = std::fs::metadata(&self.target)
            .map(|m| m.permissions())
            .unwrap_or_else(|_| self.fresh.clone());
        std::fs::set_permissions(&self.temp, permissions)?;
        std::fs::rename(&self.temp, &self.target)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Replacement {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

#[cfg(unix)]
fn owner_only(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn owner_only(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn replaces_keeping_permissions_and_cleans_up() {
        let dir = veta_testkit::TempDir::new();
        let target = dir.join("data.txt");
        std::fs::write(&target, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let (replacement, mut file) = Replacement::create(&target).unwrap();
        file.write_all(b"new").unwrap();
        drop(file);
        replacement.commit().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640, "the file keeps its permissions");
        }
        // Dropped before committing: the target stays, the temp file goes.
        let (replacement, _file) = Replacement::create(&target).unwrap();
        drop(replacement);
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_written_through() {
        let dir = veta_testkit::TempDir::new();
        let real = dir.join("real.txt");
        let link = dir.join("link.txt");
        std::fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (replacement, mut file) = Replacement::create(&link).unwrap();
        file.write_all(b"new").unwrap();
        replacement.commit().unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }
}
