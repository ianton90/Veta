//! Persisted app settings (`config.toml` in Veta's config directory).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use veta_core::io::DEFAULT_MEMORY_BUDGET;

use crate::theme::{self, Mode};

/// Maximum entries kept in [`Config::recent_files`].
pub const MAX_RECENT: usize = 10;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub appearance: Appearance,
    pub data: Data,
    pub recent_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub mode: ModePreference,
    /// Theme used in light mode, by name.
    pub light_theme: String,
    /// Theme used in dark mode, by name.
    pub dark_theme: String,
    /// Overrides the theme's font family. Applied on restart.
    pub font_family: Option<String>,
    /// Overrides the theme's font size. Applied on restart.
    pub font_size: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ModePreference {
    pub const ALL: [ModePreference; 3] = [Self::System, Self::Light, Self::Dark];

    /// The mode to use given the OS mode (`None` if unknown).
    pub fn resolve(self, system: Option<Mode>) -> Mode {
        match self {
            Self::Light => Mode::Light,
            Self::Dark => Mode::Dark,
            Self::System => system.unwrap_or(Mode::Light),
        }
    }
}

impl std::fmt::Display for ModePreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::System => "Follow system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Data {
    /// Files up to this uncompressed size load into memory; larger files are
    /// paged with a cache of this size.
    pub memory_budget_mib: u64,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: ModePreference::System,
            light_theme: theme::DEFAULT_LIGHT.into(),
            dark_theme: theme::DEFAULT_DARK.into(),
            font_family: None,
            font_size: None,
        }
    }
}

impl Default for Data {
    fn default() -> Self {
        Self {
            memory_budget_mib: DEFAULT_MEMORY_BUDGET as u64 / MIB,
        }
    }
}

impl Config {
    pub fn memory_budget(&self) -> usize {
        usize::try_from(self.data.memory_budget_mib.saturating_mul(MIB)).unwrap_or(usize::MAX)
    }

    /// Moves `path` to the front of the recent files list.
    pub fn add_recent(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(MAX_RECENT);
    }

    /// Reads the config file. A missing file gives the defaults; a broken one
    /// gives the defaults and an error message.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => (config, None),
                Err(e) => (
                    Self::default(),
                    Some(format!("Settings file {}: {}", path.display(), e.message())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!("Settings file {}: {e}", path.display())),
            ),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Veta's config directory: `$VETA_CONFIG_DIR` if set, otherwise the OS
/// convention (`%APPDATA%\veta`, `~/Library/Application Support/veta`,
/// `$XDG_CONFIG_HOME/veta` or `~/.config/veta`).
pub fn config_dir() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = env("VETA_CONFIG_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("veta"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_testkit::TempDir;

    #[test]
    fn round_trips_through_toml() {
        let dir = TempDir::new();
        let path = dir.join("sub").join("config.toml");
        let mut config = Config::default();
        config.appearance.mode = ModePreference::Dark;
        config.appearance.font_size = Some(16.0);
        config.data.memory_budget_mib = 256;
        config.recent_files.push(PathBuf::from("/data/a.parquet"));
        config.save(&path).unwrap();

        let (loaded, error) = Config::load(&path);
        assert_eq!(error, None);
        assert_eq!(loaded, config);
        assert_eq!(loaded.memory_budget(), 256 * 1024 * 1024);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let (config, error) = Config::load(Path::new("/nonexistent/config.toml"));
        assert_eq!(config, Config::default());
        assert_eq!(error, None);
    }

    #[test]
    fn partial_file_fills_defaults_and_broken_file_reports() {
        let dir = TempDir::new();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[appearance]\nmode = \"light\"\n").unwrap();
        let (config, error) = Config::load(&path);
        assert_eq!(error, None);
        assert_eq!(config.appearance.mode, ModePreference::Light);
        assert_eq!(config.appearance.dark_theme, theme::DEFAULT_DARK);

        std::fs::write(&path, "[appearance]\nmode = \"purple\"\n").unwrap();
        let (config, error) = Config::load(&path);
        assert_eq!(config, Config::default());
        assert!(error.unwrap().contains("config.toml"));
    }

    #[test]
    fn recent_files_dedupe_and_cap() {
        let mut config = Config::default();
        for i in 0..12 {
            config.add_recent(Path::new(&format!("/nonexistent/{i}.parquet")));
        }
        config.add_recent(Path::new("/nonexistent/5.parquet"));
        assert_eq!(config.recent_files.len(), MAX_RECENT);
        assert_eq!(config.recent_files[0], Path::new("/nonexistent/5.parquet"));
        assert_eq!(
            config
                .recent_files
                .iter()
                .filter(|p| p.ends_with("5.parquet"))
                .count(),
            1
        );
    }

    #[test]
    fn mode_resolution() {
        assert_eq!(ModePreference::System.resolve(Some(Mode::Dark)), Mode::Dark);
        assert_eq!(ModePreference::System.resolve(None), Mode::Light);
        assert_eq!(ModePreference::Light.resolve(Some(Mode::Dark)), Mode::Light);
    }
}
