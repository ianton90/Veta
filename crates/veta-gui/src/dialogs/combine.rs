//! Columns → Struct: combine columns into a struct column.

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
            text(tr("ST_DLG_COMBINE_TITLE")).size(20),
            text(tr_with(
                "ST_DLG_COMBINE_COLUMNS",
                &[("columns", &self.columns.join(", "))]
            ))
            .color(tokens.muted_text),
            row![
                text(tr("ST_DLG_NEW_COLUMN")).width(90),
                text_input(&tr("ST_DLG_COLUMN_NAME_PLACEHOLDER"), &self.name)
                    .id(NAME_ID)
                    .on_input(CombineMessage::Name)
                    .on_submit(CombineMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text(tr("ST_DLG_COMBINE_NOTE")).color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons(
            "ST_COMMON_OK",
            CombineMessage::Cancel,
            CombineMessage::Apply,
        ));
        card(body.into(), tokens, 440.0)
    }
}

impl StepDialog for CombineDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Combine(message) = message else {
            return None;
        };
        CombineDialog::update(self, message)
            .map(|(columns, name)| Step::CombineStruct { columns, name })
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Combine(CombineMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        CombineDialog::view(self, tokens).map(StepMessage::Combine)
    }

    fn set_error(&mut self, error: String) -> Option<String> {
        CombineDialog::set_error(self, error);
        None
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
