//! Themes: colors, font and light/dark mode, loaded from TOML files.
//!
//! Two themes are built in (`assets/themes/*.toml`); users can add their own
//! in the `themes` folder of the config directory.

use std::fmt;
use std::path::Path;

use iced::theme::{Palette, palette};
use iced::{Color, Theme};
use serde::Deserialize;

const BUILT_IN: [&str; 2] = [
    include_str!("../assets/themes/veta-light.toml"),
    include_str!("../assets/themes/veta-dark.toml"),
];

pub const DEFAULT_LIGHT: &str = "Veta Light";
pub const DEFAULT_DARK: &str = "Veta Dark";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Light,
    Dark,
}

/// A loaded theme.
#[derive(Debug, Clone)]
pub struct VetaTheme {
    pub name: String,
    pub mode: Mode,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub iced: Theme,
    pub tokens: Tokens,
}

impl fmt::Display for VetaTheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl PartialEq for VetaTheme {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// Colors Veta uses beyond iced's palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub background: Color,
    pub text: Color,
    pub muted_text: Color,
    pub surface: Color,
    pub border: Color,
    pub grid_header: Color,
    pub grid_stripe: Color,
    pub selection: Color,
    pub primary: Color,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    mode: Mode,
    #[serde(default)]
    font: FontSection,
    colors: Colors,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontSection {
    family: Option<String>,
    #[serde(default = "default_font_size")]
    size: f32,
}

impl Default for FontSection {
    fn default() -> Self {
        Self {
            family: None,
            size: default_font_size(),
        }
    }
}

fn default_font_size() -> f32 {
    14.0
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Colors {
    background: Hex,
    text: Hex,
    primary: Hex,
    success: Hex,
    warning: Hex,
    danger: Hex,
    surface: Option<Hex>,
    border: Option<Hex>,
    grid_header: Option<Hex>,
    grid_stripe: Option<Hex>,
    selection: Option<Hex>,
}

/// A `#rrggbb` or `#rrggbbaa` color.
#[derive(Debug, Clone, Copy)]
struct Hex(Color);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        parse_hex(&s).map(Hex).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid color {s:?}, expected #rrggbb"))
        })
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let alpha = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color::from_rgba8(
        byte(0)?,
        byte(2)?,
        byte(4)?,
        f32::from(alpha) / 255.0,
    ))
}

/// Parses a theme from TOML text.
pub fn parse(source: &str) -> Result<VetaTheme, String> {
    let file: ThemeFile = toml::from_str(source).map_err(|e| e.message().to_owned())?;
    if file.name.trim().is_empty() {
        return Err("theme name cannot be empty".into());
    }
    if !(6.0..=48.0).contains(&file.font.size) {
        return Err(format!(
            "font size {} is out of range (6–48)",
            file.font.size
        ));
    }
    let c = &file.colors;
    let palette = Palette {
        background: c.background.0,
        text: c.text.0,
        primary: c.primary.0,
        success: c.success.0,
        warning: c.warning.0,
        danger: c.danger.0,
    };
    let extended = palette::Extended::generate(palette);
    let tokens = Tokens {
        background: palette.background,
        text: palette.text,
        muted_text: Color {
            a: 0.6,
            ..palette.text
        },
        surface: c.surface.map_or(extended.background.weak.color, |h| h.0),
        border: c.border.map_or(extended.background.strong.color, |h| h.0),
        grid_header: c
            .grid_header
            .map_or(extended.background.weak.color, |h| h.0),
        grid_stripe: c.grid_stripe.map_or(
            Color {
                a: 0.35,
                ..extended.background.weak.color
            },
            |h| h.0,
        ),
        selection: c.selection.map_or(palette.primary, |h| h.0),
        primary: palette.primary,
    };
    Ok(VetaTheme {
        iced: Theme::custom(file.name.clone(), palette),
        name: file.name,
        mode: file.mode,
        font_family: file.font.family.filter(|f| !f.trim().is_empty()),
        font_size: file.font.size,
        tokens,
    })
}

/// Built-in themes followed by the valid themes in `dir` (if any). Returns
/// the themes and one message per file that failed to load. User themes
/// replace built-in themes with the same name.
pub fn load_all(dir: Option<&Path>) -> (Vec<VetaTheme>, Vec<String>) {
    let mut themes: Vec<VetaTheme> = BUILT_IN
        .iter()
        .map(|source| parse(source).expect("built-in themes are valid"))
        .collect();
    let mut errors = Vec::new();

    let Some(dir) = dir else {
        return (themes, errors);
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (themes, errors);
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        let result = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| parse(&s));
        match result {
            Ok(theme) => {
                themes.retain(|t| t.name != theme.name);
                themes.push(theme);
            }
            Err(e) => errors.push(format!("Theme {}: {e}", path.display())),
        }
    }
    (themes, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_themes_parse() {
        let (themes, errors) = load_all(None);
        assert!(errors.is_empty());
        let names: Vec<_> = themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, [DEFAULT_LIGHT, DEFAULT_DARK]);
        assert_eq!(themes[0].mode, Mode::Light);
        assert_eq!(themes[1].mode, Mode::Dark);
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex("#ff0000"), Some(Color::from_rgb8(255, 0, 0)));
        assert_eq!(
            parse_hex("#00000080").map(|c| (c.a * 255.0).round()),
            Some(128.0)
        );
        assert_eq!(parse_hex("ff0000"), None);
        assert_eq!(parse_hex("#ff00"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }

    #[test]
    fn optional_colors_are_derived() {
        let theme = parse(
            r##"
            name = "Minimal"
            mode = "dark"
            [colors]
            background = "#000000"
            text = "#ffffff"
            primary = "#3366ff"
            success = "#00ff00"
            warning = "#ffff00"
            danger = "#ff0000"
            "##,
        )
        .unwrap();
        assert_eq!(theme.font_size, 14.0);
        assert_eq!(theme.font_family, None);
        assert_eq!(theme.tokens.selection, Color::from_rgb8(0x33, 0x66, 0xff));
    }

    #[test]
    fn invalid_themes_report_errors() {
        assert!(parse("name = 'x'").is_err());
        let bad_color = BUILT_IN[0].replace("#2f6fde", "blue");
        assert!(parse(&bad_color).unwrap_err().contains("invalid color"));
        let unknown = format!("{}\nextra = 1", BUILT_IN[0]);
        assert!(parse(&unknown).is_err());
    }

    #[test]
    fn user_themes_load_and_override() {
        let dir = veta_testkit::TempDir::new();
        std::fs::write(
            dir.join("mine.toml"),
            BUILT_IN[0].replace("Veta Light", "Mine"),
        )
        .unwrap();
        std::fs::write(
            dir.join("override.toml"),
            BUILT_IN[1].replace("#5b9bff", "#ff00ff"),
        )
        .unwrap();
        std::fs::write(dir.join("broken.toml"), "nope").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

        let (themes, errors) = load_all(Some(dir.path()));
        let names: Vec<_> = themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, [DEFAULT_LIGHT, "Mine", DEFAULT_DARK]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("broken.toml"));
        let dark = themes.iter().find(|t| t.name == DEFAULT_DARK).unwrap();
        assert_eq!(dark.tokens.primary, Color::from_rgb8(0xff, 0x00, 0xff));
    }
}
