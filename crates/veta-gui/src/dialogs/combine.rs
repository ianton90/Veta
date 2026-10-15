//! Combine columns into a struct column.

use super::*;
use iced::widget::column;

/// Combine columns into a struct column.
#[derive(Debug, Clone, PartialEq)]
pub struct CombineDialog {
    columns: Vec<String>,
    name: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CombineMessage {
    Name(String),
    Apply,
    Cancel,
}

impl CombineDialog {
    pub fn new(columns: Vec<String>, name: String) -> Self {
        Self {
            columns,
            name,
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// The columns and struct name, on Apply.
    pub fn update(&mut self, message: CombineMessage) -> Option<(Vec<String>, String)> {
        self.error = None;
        match message {
            CombineMessage::Name(n) => self.name = n,
            CombineMessage::Cancel => {}
            CombineMessage::Apply => {
                return Some((self.columns.clone(), self.name.trim().to_owned()));
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, CombineMessage> {
        let mut body = column![
            text("Combine into struct").size(20),
            text(self.columns.join(", ")).color(tokens.muted_text),
            row![
                text("New column").width(90),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(CombineMessage::Name)
                    .on_submit(CombineMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text("The columns become its fields, in this order.").color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", CombineMessage::Cancel, CombineMessage::Apply));
        card(body.into(), tokens, 440.0)
    }
}
