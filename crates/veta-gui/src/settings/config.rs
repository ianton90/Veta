//! Persisted app settings (`config.toml` in Veta's config directory).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use veta_core::WriterSettings;

use crate::settings::theme::{self, Mode};

/// "Settings file <path>: <error>".
fn file_error(path: &Path, error: &dyn std::fmt::Display) -> String {
    crate::i18n::tr_with(
        "ST_MSG_SETTINGS_FILE_ERROR",
        &[("path", &path.display()), ("error", error)],
    )
}

/// Maximum entries kept in [`Config::recent_files`].
pub const MAX_RECENT: usize = 10;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub appearance: Appearance,
    pub data: Data,
    /// Writer settings for new and imported files.
    pub new_files: NewFiles,
    pub recent_files: Vec<PathBuf>,
}

/// Writer settings given to new and imported files, as text (as shown in
/// the app, e.g. `zstd(3)`); invalid values fall back to the defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NewFiles {
    pub compression: String,
    pub dictionary: bool,
    pub statistics: String,
    pub format_version: String,
    pub row_group_rows: usize,
}

impl Default for NewFiles {
    fn default() -> Self {
        Self::from_settings(&WriterSettings::default())
    }
}

impl NewFiles {
    pub fn from_settings(settings: &WriterSettings) -> Self {
        let d = &settings.default_column;
        Self {
            compression: d.compression.to_string(),
            dictionary: d.dictionary,
            statistics: d.statistics.to_string(),
            format_version: settings.format_version.to_string(),
            row_group_rows: settings.max_row_group_rows,
        }
    }

    pub fn settings(&self) -> WriterSettings {
        let mut settings = WriterSettings::default();
        let d = &mut settings.default_column;
        if let Ok(c) = self.compression.parse() {
            d.compression = c;
        }
        d.dictionary = self.dictionary;
        if let Ok(s) = self.statistics.parse() {
            d.statistics = s;
        }
        if let Ok(v) = self.format_version.parse() {
            settings.format_version = v;
        }
        if self.row_group_rows > 0 {
            settings.max_row_group_rows = self.row_group_rows;
        }
        settings
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub mode: ModePreference,
    /// Theme used in light mode, by name.
    pub light_theme: String,
    /// Theme used in dark mode, by name.
    pub dark_theme: String,
    /// Overrides the theme's font family.
    pub font_family: Option<String>,
    /// Overrides the theme's font size, for the interface.
    pub font_size: Option<f32>,
    /// Text size of the table; `None` is the interface's.
    pub table_font_size: Option<f32>,
    /// Zoom of the table, in percent (View → Zoom).
    pub zoom: u16,
    /// Language of the app, by locale (`es-ES`); `None` follows the system.
    pub language: Option<String>,
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
        f.write_str(&crate::i18n::tr(match self {
            Self::System => "ST_SETTINGS_MODE_SYSTEM",
            Self::Light => "ST_SETTINGS_MODE_LIGHT",
            Self::Dark => "ST_SETTINGS_MODE_DARK",
        }))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Data {
    /// Memory Veta may use for data (see `veta_core::memory`); `None` is the
    /// default, half of the machine's RAM.
    pub memory_budget_mib: Option<u64>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: ModePreference::System,
            light_theme: theme::DEFAULT_LIGHT.into(),
            dark_theme: theme::DEFAULT_DARK.into(),
            font_family: None,
            font_size: None,
            table_font_size: None,
            zoom: 100,
            language: None,
        }
    }
}

impl Config {
    /// The memory budget set, in bytes; `None` for the default.
    pub fn memory_budget(&self) -> Option<usize> {
        self.data
            .memory_budget_mib
            .map(|mib| usize::try_from(mib.saturating_mul(MIB)).unwrap_or(usize::MAX))
    }

    /// Makes the memory budget the one Veta uses.
    pub fn apply_memory_budget(&self) {
        veta_core::memory::set_budget(self.memory_budget());
    }

    /// Moves `path` to the front of the recent files list.
    /// The language to show: the chosen one if Veta has it, else the
    /// closest to the system's.
    pub fn language(&self) -> &'static str {
        let chosen = self.appearance.language.clone();
        let locale = chosen.or_else(crate::platform::locale::system_locale);
        crate::i18n::closest_language(&locale.unwrap_or_default())
    }

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
                Err(e) => (Self::default(), Some(file_error(path, &e.message()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(file_error(path, &e))),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let io = |e| format!("{}: {}", path.display(), crate::i18n::errors::io_text(e));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(path, text).map_err(io)
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
        config.data.memory_budget_mib = Some(256);
        config.recent_files.push(PathBuf::from("/data/a.parquet"));
        config.save(&path).unwrap();

        let (loaded, error) = Config::load(&path);
        assert_eq!(error, None);
        assert_eq!(loaded, config);
        assert_eq!(loaded.memory_budget(), Some(256 * 1024 * 1024));
        assert_eq!(Config::default().memory_budget(), None, "half of RAM");
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
