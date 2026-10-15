//! Edit a nested cell's value as JSON.

use super::*;
use iced::widget::column;

/// Edit a nested cell's value as JSON in a multi-line editor.
#[derive(Debug)]
pub struct JsonDialog {
    pub row: usize,
    pub column: String,
    content: iced::widget::text_editor::Content,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum JsonMessage {
    Edit(iced::widget::text_editor::Action),
    Apply,
    Cancel,
}

impl JsonDialog {
    /// `json` is the cell's value (`None` for null), shown indented.
    pub fn new(row: usize, column: String, json: Option<&str>) -> Self {
        let text = veta_core::values::pretty_json(json.unwrap_or("null"));
        Self {
            row,
            column,
            content: iced::widget::text_editor::Content::with_text(&text),
            error: None,
        }
    }

    /// Handles a message. On apply, returns the value as compact JSON
    /// (`None` inside for null).
    pub fn update(&mut self, message: JsonMessage) -> Option<Option<String>> {
        match message {
            JsonMessage::Edit(action) => {
                self.content.perform(action);
                self.error = None;
            }
            JsonMessage::Cancel => {}
            JsonMessage::Apply => match veta_core::values::compact_json(&self.content.text()) {
                Ok(value) => return Some(value),
                Err(e) => self.error = Some(e.to_string()),
            },
        }
        None
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, JsonMessage> {
        let mut body = column![
            text(format!("{}, row {}", self.column, self.row + 1)).size(20),
            iced::widget::text_editor(&self.content)
                .on_action(JsonMessage::Edit)
                .font(iced::Font::MONOSPACE)
                .height(Length::Fixed(380.0)),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("Apply", JsonMessage::Cancel, JsonMessage::Apply));
        card(body.into(), tokens, 640.0)
    }
}
