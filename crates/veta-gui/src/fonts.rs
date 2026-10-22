//! The interface font and text size, applied live (#57): iced fixes its
//! default font at startup, so every text widget is made here with the
//! current font and size instead. `App::view` sets them before drawing.
//! The table has its own size (see `GridView::set_text_size`).

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use iced::advanced::graphics::text::font_system;
use iced::widget::{Checkbox, PickList, Text, TextInput};
use iced::{Font, font};

/// The interface font and size.
#[derive(Debug, Clone, Copy)]
struct Current {
    font: Font,
    size: f32,
}

static CURRENT: RwLock<Current> = RwLock::new(Current {
    font: Font::DEFAULT,
    size: 14.0,
});

/// Sets the interface font and text size.
pub fn set(font: Font, size: f32) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Current { font, size };
}

fn current() -> Current {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// The interface font.
pub fn font() -> Font {
    current().font
}

/// The interface font in bold.
pub fn bold() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..font()
    }
}

/// The interface text size.
pub fn size() -> f32 {
    current().size
}

/// Font families to try, in order, when neither the settings nor the theme
/// name one: the platform's interface font. cosmic-text's own default
/// ("Open Sans") is often not installed.
const PREFERRED: &[&str] = if cfg!(target_os = "windows") {
    &["Segoe UI", "Tahoma", "Arial"]
} else if cfg!(target_os = "macos") {
    &[
        "SF Pro Text",
        "SF Pro",
        "Helvetica Neue",
        "Helvetica",
        "Arial",
    ]
} else {
    &[
        "Noto Sans",
        "Cantarell",
        "Ubuntu",
        "DejaVu Sans",
        "Liberation Sans",
        "Open Sans",
    ]
};

/// The installed font families that can show text, sorted.
struct Installed {
    names: Vec<String>,
    /// Lower-case name → index in `names`.
    index: HashMap<String, usize>,
}

impl Installed {
    fn new(mut names: Vec<String>) -> Self {
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lowercase(), i))
            .collect();
        Self { names, index }
    }

    /// `family` as installed (any case), if it is.
    fn find(&self, family: &str) -> Option<&str> {
        let i = *self.index.get(&family.trim().to_lowercase())?;
        Some(&self.names[i])
    }

    /// The first of `preferred` that is installed.
    fn first_of(&self, preferred: &[&str]) -> Option<&str> {
        preferred.iter().find_map(|f| self.find(f))
    }
}

/// Read once from iced's font database (the fonts iced can draw). Fonts
/// installed while Veta runs show after a restart, as iced loads them once.
fn installed() -> &'static Installed {
    static INSTALLED: LazyLock<Installed> = LazyLock::new(|| Installed::new(read_installed()));
    &INSTALLED
}

/// One face per family; families whose face has no "a" (icons, emoji,
/// symbols) and hidden ones (".SF NS") are left out.
fn read_installed() -> Vec<String> {
    let mut system = font_system().write().unwrap_or_else(|e| e.into_inner());
    let raw = system.raw();
    let mut faces = HashMap::new();
    for face in raw.db().faces() {
        if let Some((name, _)) = face.families.first()
            && !name.starts_with('.')
        {
            faces.entry(name.clone()).or_insert((face.id, face.weight));
        }
    }
    faces
        .into_iter()
        .filter(|(_, (id, weight))| {
            raw.get_font(*id, *weight)
                .is_some_and(|f| f.as_swash().charmap().map('a') != 0)
        })
        .map(|(name, _)| name)
        .collect()
}

/// The installed font families, sorted, for the Settings list.
pub fn families() -> &'static [String] {
    &installed().names
}

/// `family` as installed (any case), if it is.
pub fn find(family: &str) -> Option<&'static str> {
    installed().find(family)
}

/// The font for `family`: that family if it is installed, else the
/// platform's interface font, else iced's default. A missing family stays
/// in the config, so it comes back if it is installed again.
pub fn resolve(family: Option<&str>) -> Font {
    let installed = installed();
    let name = family
        .and_then(|f| installed.find(f))
        .or_else(|| installed.first_of(PREFERRED));
    // The names live as long as the program (`installed` is static).
    name.map_or(Font::DEFAULT, Font::with_name)
}

/// [`iced::widget::text`] in the interface font and size.
pub fn text<'a>(content: impl iced::widget::text::IntoFragment<'a>) -> Text<'a> {
    iced::widget::text(content).font(font()).size(size())
}

/// [`iced::widget::text_input`] in the interface font and size.
pub fn text_input<'a, Message: Clone>(placeholder: &str, value: &str) -> TextInput<'a, Message> {
    iced::widget::text_input(placeholder, value)
        .font(font())
        .size(size())
}

/// [`iced::widget::pick_list`] in the interface font and size.
pub fn pick_list<'a, T, L, V, Message>(
    options: L,
    selected: Option<V>,
    on_selected: impl Fn(T) -> Message + 'a,
) -> PickList<'a, T, L, V, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    Message: Clone,
{
    iced::widget::pick_list(options, selected, on_selected)
        .font(font())
        .text_size(size())
}

/// [`iced::widget::checkbox`] in the interface font and size.
pub fn checkbox<'a, Message>(is_checked: bool) -> Checkbox<'a, Message> {
    iced::widget::checkbox(is_checked)
        .font(font())
        .text_size(size())
}

/// iced's text widget module, beside [`text`] (for `text::Wrapping`, …).
pub mod text {
    pub use iced::widget::text::*;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_families_in_any_case() {
        let installed = Installed::new(vec!["Noto Sans".into(), "Arial".into(), "Arial".into()]);
        assert_eq!(installed.names, ["Arial", "Noto Sans"]);
        assert_eq!(installed.find(" noto SANS "), Some("Noto Sans"));
        assert_eq!(installed.find("Gone"), None);
        assert_eq!(installed.first_of(&["Gone", "arial"]), Some("Arial"));
        assert_eq!(installed.first_of(&["Gone"]), None);
    }

    #[test]
    fn missing_families_fall_back() {
        let fallback = resolve(None);
        assert_eq!(resolve(Some("No Such Font 123")), fallback);
        if let Some(first) = families().first() {
            assert_eq!(resolve(Some(first)), Font::with_name(first));
            assert_eq!(find(&first.to_uppercase()), Some(first.as_str()));
        }
    }
}
