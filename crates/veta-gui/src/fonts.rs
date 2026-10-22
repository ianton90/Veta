//! The interface font and text size, applied live (#57): iced fixes its
//! default font at startup, so every text widget is made here with the
//! current font and size instead. `App::view` sets them before drawing.
//! The table has its own size (see `GridView::set_text_size`).

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, RwLock};

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

/// The font named `family`, or the default one. iced needs `'static` names:
/// each family is leaked once and reused.
pub fn named(family: Option<&str>) -> Font {
    static NAMES: LazyLock<Mutex<HashMap<String, &'static str>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let Some(family) = family.map(str::trim).filter(|f| !f.is_empty()) else {
        return Font::DEFAULT;
    };
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let name = *names
        .entry(family.to_owned())
        .or_insert_with(|| Box::leak(family.to_owned().into_boxed_str()));
    Font::with_name(name)
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
    fn families_are_named_once() {
        assert_eq!(named(None), Font::DEFAULT);
        assert_eq!(named(Some("  ")), Font::DEFAULT);
        assert_eq!(named(Some("Noto Sans")), named(Some("Noto Sans")));
        assert_eq!(
            named(Some("Noto Sans")).family,
            font::Family::Name("Noto Sans")
        );
    }
}
