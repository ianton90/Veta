//! Replace values in a column.

use super::*;
use iced::widget::column;

/// Replace values in a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplaceDialog {
    column: String,
    text_column: bool,
    find: String,
    find_null: bool,
    with: String,
    with_null: bool,
    whole_cell: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ReplaceMessage {
    Find(String),
    FindNull(bool),
    With(String),
    WithNull(bool),
    WholeCell(bool),
    Apply,
    Cancel,
}

impl ReplaceDialog {
    pub fn new(column: String, text_column: bool, replace: Option<&Replace>) -> Self {
        Self {
            column,
            text_column,
            find: replace.and_then(|r| r.find.clone()).unwrap_or_default(),
            find_null: replace.is_some_and(|r| r.find.is_none()),
            with: replace.and_then(|r| r.with.clone()).unwrap_or_default(),
            with_null: replace.is_some_and(|r| r.with.is_none()),
            whole_cell: replace.is_some_and(|r| r.whole_cell),
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: ReplaceMessage) -> Option<Replace> {
        self.error = None;
        match message {
            ReplaceMessage::Find(t) => self.find = t,
            ReplaceMessage::FindNull(on) => self.find_null = on,
            ReplaceMessage::With(t) => self.with = t,
            ReplaceMessage::WithNull(on) => self.with_null = on,
            ReplaceMessage::WholeCell(on) => self.whole_cell = on,
            ReplaceMessage::Cancel => {}
            ReplaceMessage::Apply => {
                return Some(Replace {
                    column: self.column.clone(),
                    find: (!self.find_null).then(|| self.find.clone()),
                    with: (!self.with_null).then(|| self.with.clone()),
                    whole_cell: self.whole_cell || !self.text_column,
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ReplaceMessage> {
        let mut find = text_input("Value to find", &self.find).id(NAME_ID);
        if !self.find_null {
            find = find
                .on_input(ReplaceMessage::Find)
                .on_submit(ReplaceMessage::Apply);
        }
        let mut with = text_input("Replace with", &self.with);
        if !self.with_null {
            with = with
                .on_input(ReplaceMessage::With)
                .on_submit(ReplaceMessage::Apply);
        }
        let mut body = column![
            text(format!("Replace values in {}", self.column)).size(20),
            row![
                text("Find").width(80),
                find,
                checkbox(self.find_null)
                    .label("null")
                    .on_toggle(ReplaceMessage::FindNull),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text("Replace with").width(80),
                with,
                checkbox(self.with_null)
                    .label("null")
                    .on_toggle(ReplaceMessage::WithNull),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(14);
        if self.text_column && !self.find_null {
            body = body.push(
                checkbox(self.whole_cell)
                    .label("Match entire cell contents")
                    .on_toggle(ReplaceMessage::WholeCell),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        } else if !self.text_column {
            body = body.push(
                text("Values are read as the column's type and match whole cells.")
                    .color(tokens.muted_text),
            );
        }
        body = body.push(buttons("OK", ReplaceMessage::Cancel, ReplaceMessage::Apply));
        card(body.into(), tokens, 480.0)
    }
}
