//! Translations: one shared object, a map tag → language → text, read from
//! `translations/translations_<locale>.json` (built into the app). Tags of
//! static texts start with `ST_`; texts with values hold `{name}`
//! placeholders. See DECISIONS 51.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{LazyLock, RwLock, RwLockReadGuard};

/// The languages Veta ships: locale and its translations file.
const FILES: [(&str, &str); 2] = [
    (
        "en-GB",
        include_str!("../../../translations/translations_en-GB.json"),
    ),
    (
        "es-ES",
        include_str!("../../../translations/translations_es-ES.json"),
    ),
];

/// Language used when a text is missing in the current one.
pub const FALLBACK: &str = "en-GB";

/// Tag of each language's own name ("English (UK)", "Español (España)").
const LANGUAGE_NAME: &str = "ST_LANGUAGE_NAME";

static INSTANCE: LazyLock<RwLock<Translations>> =
    LazyLock::new(|| RwLock::new(Translations::load(FALLBACK)));

#[derive(Debug, Default)]
pub struct Translations {
    /// tag → language → text.
    texts: HashMap<String, HashMap<String, String>>,
    language: String,
}

impl Translations {
    /// Reads every translations file; `language` becomes the current one.
    pub fn load(language: &str) -> Self {
        let mut texts: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (locale, file) in FILES {
            let file: HashMap<String, String> =
                serde_json::from_str(file).expect("built-in translations are valid JSON");
            for (tag, text) in file {
                texts
                    .entry(tag)
                    .or_default()
                    .insert(locale.to_owned(), text);
            }
        }
        Self {
            texts,
            language: language.to_owned(),
        }
    }

    /// The shared translations.
    pub fn global() -> RwLockReadGuard<'static, Translations> {
        INSTANCE.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the translations again and switches to `language`.
    pub fn set_language(language: &str) {
        let loaded = Translations::load(language);
        *INSTANCE.write().unwrap_or_else(|e| e.into_inner()) = loaded;
    }

    /// The current language's locale, e.g. `es-ES`.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// `tag`'s text in `language`, if it has one.
    pub fn text(&self, tag: &str, language: &str) -> Option<&str> {
        self.texts.get(tag)?.get(language).map(String::as_str)
    }

    /// `tag`'s text in the current language, else in English, else the tag.
    pub fn get<'a>(&'a self, tag: &'a str) -> &'a str {
        self.text(tag, &self.language)
            .or_else(|| self.text(tag, FALLBACK))
            .unwrap_or(tag)
    }

    /// The languages Veta ships: locale and the language's own name.
    pub fn languages(&self) -> Vec<(&'static str, String)> {
        FILES
            .iter()
            .map(|(locale, _)| {
                let name = self.text(LANGUAGE_NAME, locale).unwrap_or(locale);
                (*locale, name.to_owned())
            })
            .collect()
    }

    /// Every tag, sorted (for tests and tools).
    pub fn tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = self.texts.keys().map(String::as_str).collect();
        tags.sort_unstable();
        tags
    }
}

/// `tag`'s text in the current language.
pub fn tr(tag: &str) -> String {
    Translations::global().get(tag).to_owned()
}

/// `tag`'s text with each `{name}` replaced by its value:
/// `tr_with("ST_MENU_DELETE_ROWS", &[("n", &3)])` → "Delete 3 rows".
pub fn tr_with(tag: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut text = tr(tag);
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

/// The supported language closest to `locale` (e.g. a system locale such as
/// `es_MX.UTF-8`): the same language, else English.
pub fn closest_language(locale: &str) -> &'static str {
    let wanted = locale.get(..2).unwrap_or_default().to_ascii_lowercase();
    FILES
        .iter()
        .map(|(l, _)| *l)
        .find(|l| l[..2] == wanted)
        .unwrap_or(FALLBACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_tag() {
        for (locale, file) in FILES {
            let file: HashMap<String, String> = serde_json::from_str(file).expect(locale);
            let translations = Translations::load(FALLBACK);
            let missing: Vec<_> = translations
                .tags()
                .into_iter()
                .filter(|tag| !file.contains_key(*tag))
                .collect();
            assert!(missing.is_empty(), "{locale} lacks {missing:?}");
        }
    }

    #[test]
    fn tags_follow_the_naming_rules() {
        // Static texts start with ST_; upper case, digits and underscores.
        for tag in Translations::load(FALLBACK).tags() {
            assert!(tag.starts_with("ST_"), "{tag}");
            assert!(
                tag.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{tag}"
            );
        }
    }

    #[test]
    fn falls_back_to_english_then_the_tag() {
        let mut t = Translations::load("es-ES");
        assert_eq!(t.get(LANGUAGE_NAME), "Español (España)");
        t.texts.get_mut(LANGUAGE_NAME).expect("tag").remove("es-ES");
        assert_eq!(t.get(LANGUAGE_NAME), "English (UK)");
        assert_eq!(t.get("ST_NO_SUCH_TAG"), "ST_NO_SUCH_TAG");
    }

    #[test]
    fn placeholders_and_languages() {
        assert_eq!(
            Translations::load(FALLBACK).languages(),
            [
                ("en-GB", "English (UK)".to_owned()),
                ("es-ES", "Español (España)".to_owned())
            ]
        );
        assert_eq!(closest_language("es_MX.UTF-8"), "es-ES");
        assert_eq!(closest_language("de-DE"), "en-GB");
        assert_eq!(closest_language(""), "en-GB");
        // An unknown tag shows itself, with its values filled in.
        assert_eq!(
            tr_with("ST_X {n} {c}", &[("n", &3), ("c", &"a")]),
            "ST_X 3 a"
        );
    }
}
