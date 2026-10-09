//! Settings and About dialogs.

use std::path::Path;

use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{Alignment, Border, Element, Length, Theme};

use crate::config::{Config, ModePreference};
use crate::theme::{Mode, Tokens, VetaTheme};

/// Settings being edited; applied to [`Config`] on save.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    mode: ModePreference,
    light_theme: String,
    dark_theme: String,
    font_family: String,
    font_size: String,
    memory_mib: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Mode(ModePreference),
    LightTheme(String),
    DarkTheme(String),
    FontFamily(String),
    FontSize(String),
    Memory(String),
    Save,
    Cancel,
}

/// Result of a settings message.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Editing,
    Saved(Config),
    Cancelled,
}

impl Draft {
    pub fn new(config: &Config) -> Self {
        let a = &config.appearance;
        Self {
            mode: a.mode,
            light_theme: a.light_theme.clone(),
            dark_theme: a.dark_theme.clone(),
            font_family: a.font_family.clone().unwrap_or_default(),
            font_size: a.font_size.map(|s| s.to_string()).unwrap_or_default(),
            memory_mib: config.data.memory_budget_mib.to_string(),
            error: None,
        }
    }

    pub fn update(&mut self, message: SettingsMessage, config: &Config) -> Outcome {
        match message {
            SettingsMessage::Mode(mode) => self.mode = mode,
            SettingsMessage::LightTheme(name) => self.light_theme = name,
            SettingsMessage::DarkTheme(name) => self.dark_theme = name,
            SettingsMessage::FontFamily(family) => self.font_family = family,
            SettingsMessage::FontSize(size) => self.font_size = size,
            SettingsMessage::Memory(mib) => self.memory_mib = mib,
            SettingsMessage::Cancel => return Outcome::Cancelled,
            SettingsMessage::Save => match self.apply(config) {
                Ok(config) => return Outcome::Saved(config),
                Err(e) => self.error = Some(e),
            },
        }
        Outcome::Editing
    }

    fn apply(&self, config: &Config) -> Result<Config, String> {
        let font_size = match self.font_size.trim() {
            "" => None,
            s => {
                let size: f32 = s
                    .parse()
                    .map_err(|_| format!("Font size {s:?} is not a number."))?;
                if !(6.0..=48.0).contains(&size) {
                    return Err("Font size must be between 6 and 48.".into());
                }
                Some(size)
            }
        };
        let memory: u64 =
            self.memory_mib.trim().parse().map_err(|_| {
                format!("Memory budget {:?} is not a whole number.", self.memory_mib)
            })?;
        if memory < 16 {
            return Err("Memory budget must be at least 16 MiB.".into());
        }

        let mut config = config.clone();
        let a = &mut config.appearance;
        a.mode = self.mode;
        a.light_theme = self.light_theme.clone();
        a.dark_theme = self.dark_theme.clone();
        a.font_family = Some(self.font_family.trim().to_owned()).filter(|f| !f.is_empty());
        a.font_size = font_size;
        config.data.memory_budget_mib = memory;
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

    let label = |s: &'a str| text(s).width(160);
    let field_row = |l: &'a str, input: Element<'a, SettingsMessage>| {
        row![label(l), input].spacing(12).align_y(Alignment::Center)
    };
    let note = |s: String| text(s).color(tokens.muted_text);

    let appearance = column![
        section("Appearance"),
        field_row(
            "Mode",
            pick_list(ModePreference::ALL, Some(draft.mode), SettingsMessage::Mode)
                .width(220)
                .into()
        ),
        field_row(
            "Light theme",
            pick_list(
                names(Mode::Light),
                Some(draft.light_theme.clone()),
                SettingsMessage::LightTheme
            )
            .width(220)
            .into()
        ),
        field_row(
            "Dark theme",
            pick_list(
                names(Mode::Dark),
                Some(draft.dark_theme.clone()),
                SettingsMessage::DarkTheme
            )
            .width(220)
            .into()
        ),
        field_row(
            "Font family",
            text_input("Theme default", &draft.font_family)
                .on_input(SettingsMessage::FontFamily)
                .width(220)
                .into()
        ),
        field_row(
            "Font size",
            text_input("Theme default", &draft.font_size)
                .on_input(SettingsMessage::FontSize)
                .width(220)
                .into()
        ),
        note("Font changes apply after restarting Veta.".into()),
        note(match themes_dir {
            Some(dir) => format!("Custom themes: put .toml files in {}", dir.display()),
            None => "Custom themes: no config directory found.".into(),
        }),
    ]
    .spacing(10);

    let data = column![
        section("Data"),
        field_row(
            "Memory budget (MiB)",
            text_input("1024", &draft.memory_mib)
                .on_input(SettingsMessage::Memory)
                .width(220)
                .into()
        ),
        note(
            "Files up to this size open in memory; larger files are read from disk \
             as needed. Applies to files opened from now on."
                .into()
        ),
    ]
    .spacing(10);

    let mut body = column![text("Settings").size(20), appearance, data].spacing(22);
    if let Some(error) = &draft.error {
        body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
    }
    body = body.push(
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text("Cancel"))
                .style(button::secondary)
                .on_press(SettingsMessage::Cancel),
            button(text("Save")).on_press(SettingsMessage::Save),
        ]
        .spacing(8),
    );

    card(body.into(), tokens, 520.0)
}

pub fn about<'a, Message: Clone + 'a>(close: Message, tokens: Tokens) -> Element<'a, Message> {
    let body = column![
        text("Veta").size(22),
        text(format!("Version {}", env!("CARGO_PKG_VERSION"))),
        text("A Parquet viewer and editor."),
        text("Licensed under Apache-2.0. Icons from Lucide (ISC License).")
            .color(tokens.muted_text),
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text("Close")).on_press(close),
        ],
    ]
    .spacing(12);
    card(body.into(), tokens, 380.0)
}

fn section<'a, Message: 'a>(title: &'a str) -> Element<'a, Message> {
    text(title).size(15).font(crate::bold()).into()
}

fn card<'a, Message: 'a>(
    content: Element<'a, Message>,
    tokens: Tokens,
    width: f32,
) -> Element<'a, Message> {
    container(content)
        .padding(24)
        .width(width)
        .style(move |_theme: &Theme| {
            container::Style::default()
                .background(tokens.background)
                .color(tokens.text)
                .border(Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 8.0.into(),
                })
                .shadow(iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25),
                    offset: iced::Vector::new(0.0, 6.0),
                    blur_radius: 24.0,
                })
        })
        .into()
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
        assert_eq!(saved.data.memory_budget_mib, 512);
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
}
