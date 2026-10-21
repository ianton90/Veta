//! Translations: switching language, and every tag the code uses exists.
//! Its own test binary, as switching language changes texts process-wide.

use std::path::Path;

use veta_gui::i18n::{FALLBACK, Translations, tr, tr_with};

#[test]
fn switching_language_reloads_the_texts() {
    assert_eq!(tr("ST_SETTINGS_LANGUAGE"), "Language");
    Translations::set_language("es-ES");
    assert_eq!(Translations::global().language(), "es-ES");
    assert_eq!(tr("ST_SETTINGS_LANGUAGE"), "Idioma");
    assert_eq!(tr_with("ST_NOT_A_TAG {n}", &[("n", &2)]), "ST_NOT_A_TAG 2");
    Translations::set_language(FALLBACK);
    assert_eq!(tr("ST_SETTINGS_LANGUAGE"), "Language");
}

#[test]
fn every_tag_in_the_code_has_a_text() {
    let translations = Translations::load(FALLBACK);
    let known = translations.tags();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut missing = Vec::new();
    {
        for (file, tag) in tags_in(&src) {
            if !known.contains(&tag.as_str()) {
                missing.push(format!("{tag} ({file})"));
            }
        }
    }
    assert!(missing.is_empty(), "tags without a text: {missing:#?}");
}

/// Every `"ST_…"` literal under `dir`, with the file it is in (not in
/// `i18n/mod.rs`, whose examples and tests name made-up tags).
fn tags_in(dir: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(tags_in(&path));
        } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n/mod.rs") {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for part in text.split("\"ST_").skip(1) {
                let name: String = part
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                    .collect();
                // Only whole literals: "ST_NAME".
                if !name.is_empty() && part[name.len()..].starts_with('"') {
                    found.push((path.display().to_string(), format!("ST_{name}")));
                }
            }
        }
    }
    found
}
