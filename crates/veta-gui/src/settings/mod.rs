//! Settings and About dialogs, the persisted config and themes.

pub mod config;
pub mod theme;

use crate::fonts::{checkbox, pick_list, text, text_input};
use std::path::Path;

use crate::i18n::{Translations, tr, tr_with};
use iced::widget::{button, column, row};
use iced::{Alignment, Element, Length};
use veta_core::{Compression, FormatVersion, StatisticsLevel, WriterSettings};

use crate::dialogs::{Level, card, level_picker};
use crate::settings::config::{Config, ModePreference, NewFiles};
use crate::settings::theme::{Mode, Tokens, VetaTheme};

/// Settings being edited; applied to [`Config`] on save.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    language: Language,
    mode: ModePreference,
    light_theme: String,
    dark_theme: String,
    font_family: String,
    font_size: String,
    table_font_size: String,
    memory_mib: String,
    /// Shown when `memory_mib` is empty: the default budget.
    memory_placeholder: String,
    /// Defaults for new files; `row_group` is the text being typed.
    new_files: WriterSettings,
    row_group: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Language(Language),
    Mode(ModePreference),
    LightTheme(String),
    DarkTheme(String),
    FontFamily(String),
    FontSize(String),
    TableFontSize(String),
    Memory(String),
    Compression(Compression),
    Level(Level),
    Dictionary(bool),
    Statistics(StatisticsLevel),
    Version(FormatVersion),
    RowGroup(String),
    Save,
    Cancel,
}

/// A language in the picker: its locale, shown by its own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    pub locale: &'static str,
    name: String,
}

impl Language {
    pub fn all() -> Vec<Language> {
        Translations::global()
            .languages()
            .into_iter()
            .map(|(locale, name)| Language { locale, name })
            .collect()
    }

    fn of(locale: &str) -> Language {
        let all = Language::all();
        let found = all.iter().find(|l| l.locale == locale).cloned();
        found.unwrap_or_else(|| all[0].clone())
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// Result of a settings message.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Editing,
    Saved(Box<Config>),
    Cancelled,
}

impl Draft {
    pub fn new(config: &Config) -> Self {
        let a = &config.appearance;
        Self {
            language: Language::of(config.language()),
            mode: a.mode,
            light_theme: a.light_theme.clone(),
            dark_theme: a.dark_theme.clone(),
            font_family: a.font_family.clone().unwrap_or_default(),
            font_size: a.font_size.map(|s| s.to_string()).unwrap_or_default(),
            table_font_size: a.table_font_size.map(|s| s.to_string()).unwrap_or_default(),
            memory_placeholder: tr_with(
                "ST_SETTINGS_MEMORY_AUTOMATIC",
                &[(
                    "mib",
                    &(veta_core::memory::default_budget() / (1024 * 1024)),
                )],
            ),
            memory_mib: config
                .data
                .memory_budget_mib
                .map(|mib| mib.to_string())
                .unwrap_or_default(),
            new_files: config.new_files.settings(),
            row_group: config.new_files.settings().max_row_group_rows.to_string(),
            error: None,
        }
    }

    pub fn update(&mut self, message: SettingsMessage, config: &Config) -> Outcome {
        match message {
            SettingsMessage::Language(language) => self.language = language,
            SettingsMessage::Mode(mode) => self.mode = mode,
            SettingsMessage::LightTheme(name) => self.light_theme = name,
            SettingsMessage::DarkTheme(name) => self.dark_theme = name,
            SettingsMessage::FontFamily(family) => self.font_family = family,
            SettingsMessage::FontSize(size) => self.font_size = size,
            SettingsMessage::TableFontSize(size) => self.table_font_size = size,
            SettingsMessage::Memory(mib) => self.memory_mib = mib,
            SettingsMessage::Compression(c) => {
                let d = &mut self.new_files.default_column;
                let level = d
                    .compression
                    .level()
                    .filter(|l| c.level_range().is_some_and(|r| r.contains(l)));
                d.compression = c.with_level(level);
            }
            SettingsMessage::Level(level) => {
                let d = &mut self.new_files.default_column;
                d.compression = d.compression.with_level(level.0);
            }
            SettingsMessage::Dictionary(on) => self.new_files.default_column.dictionary = on,
            SettingsMessage::Statistics(s) => self.new_files.default_column.statistics = s,
            SettingsMessage::Version(v) => self.new_files.format_version = v,
            SettingsMessage::RowGroup(rows) => self.row_group = rows,
            SettingsMessage::Cancel => return Outcome::Cancelled,
            SettingsMessage::Save => match self.apply(config) {
                Ok(config) => return Outcome::Saved(Box::new(config)),
                Err(e) => self.error = Some(e),
            },
        }
        Outcome::Editing
    }

    fn apply(&self, config: &Config) -> Result<Config, String> {
        let table_font_size = parse_font_size(&self.table_font_size)?;
        let font_size = parse_font_size(&self.font_size)?;
        // Empty: the default, half of RAM.
        let memory = match self.memory_mib.trim() {
            "" => None,
            text => {
                let mib: u64 = text
                    .parse()
                    .map_err(|_| tr_with("ST_SETTINGS_MEMORY_NOT_NUMBER", &[("value", &text)]))?;
                if mib < 64 {
                    return Err(tr("ST_SETTINGS_MEMORY_MINIMUM"));
                }
                Some(mib)
            }
        };
        let row_group: usize = match self.row_group.trim().parse() {
            Ok(rows) if rows > 0 => rows,
            _ => return Err(tr("ST_SETTINGS_ROW_GROUP_INVALID")),
        };
        let mut new_files = self.new_files.clone();
        new_files.max_row_group_rows = row_group;

        let mut config = config.clone();
        let a = &mut config.appearance;
        a.language = Some(self.language.locale.to_owned());
        a.mode = self.mode;
        a.light_theme = self.light_theme.clone();
        a.dark_theme = self.dark_theme.clone();
        a.font_family = Some(self.font_family.trim().to_owned()).filter(|f| !f.is_empty());
        a.font_size = font_size;
        a.table_font_size = table_font_size;
        config.data.memory_budget_mib = memory;
        config.new_files = NewFiles::from_settings(&new_files);
        Ok(config)
    }
}

pub fn view<'a>(
    draft: &'a Draft,
    themes: &'a [VetaTheme],
    themes_dir: Option<&'a Path>,
    tokens: Tokens,
) -> Element<'a, SettingsMessage> {
    let names = |mode: Mode| -> Vec<String> {
        themes
            .iter()
            .filter(|t| t.mode == mode)
            .map(|t| t.name.clone())
            .collect()
    };

    let label = |s: String| text(s).width(160);
    let field_row = |l: String, input: Element<'a, SettingsMessage>| {
        row![label(l), input].spacing(12).align_y(Alignment::Center)
    };
    let note = |s: String| text(s).color(tokens.muted_text);

    let appearance = column![
        section(tr("ST_SETTINGS_SECTION_APPEARANCE")),
        field_row(
            tr("ST_SETTINGS_LANGUAGE"),
            pick_list(
                Language::all(),
                Some(draft.language.clone()),
                SettingsMessage::Language
            )
            .width(220)
            .into()
        ),
        field_row(
            tr("ST_SETTINGS_MODE"),
            pick_list(ModePreference::ALL, Some(draft.mode), SettingsMessage::Mode)
                .width(220)
                .into()
        ),
        field_row(
            tr("ST_SETTINGS_LIGHT_THEME"),
            pick_list(
                names(Mode::Light),
                Some(draft.light_theme.clone()),
                SettingsMessage::LightTheme
            )
            .width(220)
            .into()
        ),
        field_row(
            tr("ST_SETTINGS_DARK_THEME"),
            pick_list(
                names(Mode::Dark),
                Some(draft.dark_theme.clone()),
                SettingsMessage::DarkTheme
            )
            .width(220)
            .into()
        ),
        field_row(
            tr("ST_SETTINGS_FONT_FAMILY"),
            text_input(&tr("ST_SETTINGS_THEME_DEFAULT"), &draft.font_family)
                .on_input(SettingsMessage::FontFamily)
                .width(220)
                .into()
        ),
        field_row(
            tr("ST_SETTINGS_FONT_SIZE"),
            text_input(&tr("ST_SETTINGS_THEME_DEFAULT"), &draft.font_size)
                .on_input(SettingsMessage::FontSize)
                .width(220)
                .into()
        ),
        field_row(
            tr("ST_SETTINGS_TABLE_FONT_SIZE"),
            text_input(&tr("ST_SETTINGS_TABLE_SAME"), &draft.table_font_size)
                .on_input(SettingsMessage::TableFontSize)
                .width(220)
                .into()
        ),
        note(tr("ST_SETTINGS_ZOOM_NOTE")),
        note(match themes_dir {
            Some(dir) => tr_with("ST_SETTINGS_THEMES_DIR", &[("dir", &dir.display())]),
            None => tr("ST_SETTINGS_THEMES_NO_DIR"),
        }),
    ]
    .spacing(10);

    let data = column![
        section(tr("ST_SETTINGS_SECTION_DATA")),
        field_row(
            tr("ST_SETTINGS_MEMORY"),
            text_input(&draft.memory_placeholder, &draft.memory_mib)
                .on_input(SettingsMessage::Memory)
                .width(220)
                .into()
        ),
        note(tr("ST_SETTINGS_MEMORY_NOTE")),
    ]
    .spacing(10);

    let d = &draft.new_files.default_column;
    let new_files = column![
        section(tr("ST_SETTINGS_SECTION_NEW_FILES")),
        field_row(
            tr("ST_SETTINGS_COMPRESSION"),
            row![
                pick_list(
                    Compression::CHOICES,
                    Some(d.compression.without_level()),
                    SettingsMessage::Compression
                )
                .width(130),
                level_picker(d.compression, 82.0, SettingsMessage::Level),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        ),
        field_row(
            tr("ST_SETTINGS_STATISTICS"),
            pick_list(
                StatisticsLevel::ALL,
                Some(d.statistics),
                SettingsMessage::Statistics
            )
            .width(220)
            .into()
        ),
        field_row(
            tr("ST_SETTINGS_FORMAT_VERSION"),
            pick_list(
                FormatVersion::ALL,
                Some(draft.new_files.format_version),
                SettingsMessage::Version
            )
            .width(220)
            .into()
        ),
        field_row(
            tr("ST_SETTINGS_ROW_GROUP"),
            text_input("1048576", &draft.row_group)
                .on_input(SettingsMessage::RowGroup)
                .width(220)
                .into()
        ),
        field_row(
            tr("ST_SETTINGS_DICTIONARY"),
            checkbox(d.dictionary)
                .on_toggle(SettingsMessage::Dictionary)
                .into()
        ),
        note(tr("ST_SETTINGS_NEW_FILES_NOTE")),
    ]
    .spacing(10);

    let mut body = column![
        text(tr("ST_SETTINGS_TITLE")).size(20),
        iced::widget::scrollable(
            column![appearance, data, new_files]
                .spacing(22)
                .padding(iced::Padding::ZERO.right(18))
        )
        .height(Length::Fixed(520.0)),
    ]
    .spacing(22);
    if let Some(error) = &draft.error {
        body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
    }
    body = body.push(
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text(tr("ST_COMMON_CANCEL")))
                .style(button::secondary)
                .on_press(SettingsMessage::Cancel),
            button(text(tr("ST_COMMON_SAVE"))).on_press(SettingsMessage::Save),
        ]
        .spacing(8),
    );

    card(body.into(), tokens, 520.0)
}

pub fn about<'a, Message: Clone + 'a>(close: Message, tokens: Tokens) -> Element<'a, Message> {
    let body = column![
        text("Veta").size(22),
        text(tr_with(
            "ST_ABOUT_VERSION",
            &[("version", &veta_core::version::LONG)]
        )),
        text(tr("ST_ABOUT_TAGLINE")),
        text(tr("ST_ABOUT_LICENSE")).color(tokens.muted_text),
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text(tr("ST_COMMON_CLOSE"))).on_press(close),
        ],
    ]
    .spacing(12);
    card(body.into(), tokens, 380.0)
}

/// A font size typed in Settings: empty for the default, else 6 to 48.
fn parse_font_size(text: &str) -> Result<Option<f32>, String> {
    match text.trim() {
        "" => Ok(None),
        s => {
            let size: f32 = s
                .parse()
                .map_err(|_| tr_with("ST_SETTINGS_FONT_SIZE_NOT_NUMBER", &[("value", &s)]))?;
            if !(6.0..=48.0).contains(&size) {
                return Err(tr("ST_SETTINGS_FONT_SIZE_RANGE"));
            }
            Ok(Some(size))
        }
    }
}

fn section<'a, Message: 'a>(title: String) -> Element<'a, Message> {
    text(title).size(15).font(crate::app::bold()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_validates_and_applies() {
        let config = Config::default();
        let mut draft = Draft::new(&config);
        assert_eq!(
            draft.update(SettingsMessage::FontSize("big".into()), &config),
            Outcome::Editing
        );
        assert_eq!(
            draft.update(SettingsMessage::Save, &config),
            Outcome::Editing
        );
        assert!(draft.error.as_deref().unwrap().contains("not a number"));

        draft.update(SettingsMessage::FontSize("16".into()), &config);
        draft.update(SettingsMessage::Memory("512".into()), &config);
        draft.update(SettingsMessage::Mode(ModePreference::Dark), &config);
        draft.update(SettingsMessage::FontFamily("  ".into()), &config);
        let Outcome::Saved(saved) = draft.update(SettingsMessage::Save, &config) else {
            panic!("expected save");
        };
        assert_eq!(saved.appearance.font_size, Some(16.0));
        assert_eq!(saved.appearance.font_family, None);
        assert_eq!(saved.appearance.mode, ModePreference::Dark);
        assert_eq!(saved.data.memory_budget_mib, Some(512));
    }

    #[test]
    fn language_is_saved_by_locale() {
        let config = Config::default();
        let mut draft = Draft::new(&config);
        let spanish = Language::of("es-ES");
        assert_eq!(spanish.to_string(), "Español (España)");
        draft.update(SettingsMessage::Language(spanish), &config);
        let Outcome::Saved(saved) = draft.update(SettingsMessage::Save, &config) else {
            panic!("expected save");
        };
        assert_eq!(saved.appearance.language.as_deref(), Some("es-ES"));
        assert_eq!(saved.language(), "es-ES");
    }

    #[test]
    fn memory_budget_has_minimum() {
        let config = Config::default();
        let mut draft = Draft::new(&config);
        draft.update(SettingsMessage::Memory("4".into()), &config);
        assert_eq!(
            draft.update(SettingsMessage::Save, &config),
            Outcome::Editing
        );
        assert!(draft.error.is_some());
    }

    #[test]
    fn cancel() {
        let config = Config::default();
        let mut draft = Draft::new(&config);
        assert_eq!(
            draft.update(SettingsMessage::Cancel, &config),
            Outcome::Cancelled
        );
    }

    #[test]
    fn writer_defaults_for_new_files() {
        let config = Config::default();
        let mut draft = Draft::new(&config);
        draft.update(
            SettingsMessage::Compression(Compression::Zstd(None)),
            &config,
        );
        draft.update(SettingsMessage::Level(Level(Some(5))), &config);
        draft.update(SettingsMessage::Statistics(StatisticsLevel::Chunk), &config);
        draft.update(SettingsMessage::RowGroup("0".into()), &config);
        assert_eq!(
            draft.update(SettingsMessage::Save, &config),
            Outcome::Editing
        );
        draft.update(SettingsMessage::RowGroup("5000".into()), &config);
        let Outcome::Saved(saved) = draft.update(SettingsMessage::Save, &config) else {
            panic!("expected save");
        };
        assert_eq!(saved.new_files.compression, "zstd(5)");
        let settings = saved.new_files.settings();
        assert_eq!(
            settings.default_column.compression,
            Compression::Zstd(Some(5))
        );
        assert_eq!(settings.default_column.statistics, StatisticsLevel::Chunk);
        assert_eq!(settings.max_row_group_rows, 5000);
        // Round trip through config.toml text.
        let text = toml::to_string(&saved).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.new_files, saved.new_files);
    }
}
