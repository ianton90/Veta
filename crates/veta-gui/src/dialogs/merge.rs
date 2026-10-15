//! Merge columns into one.

use super::*;
use iced::widget::column;

/// Merge columns into one.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeDialog {
    columns: Vec<String>,
    separator: String,
    name: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MergeMessage {
    Separator(String),
    Name(String),
    Apply,
    Cancel,
}

impl MergeDialog {
    pub fn new(columns: Vec<String>, merge: Option<&Merge>) -> Self {
        Self {
            separator: merge.map_or_else(|| " ".into(), |m| m.separator.clone()),
            name: merge.map_or_else(|| "Merged".into(), |m| m.name.clone()),
            columns,
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: MergeMessage) -> Option<Merge> {
        self.error = None;
        match message {
            MergeMessage::Separator(s) => self.separator = s,
            MergeMessage::Name(n) => self.name = n,
            MergeMessage::Cancel => {}
            MergeMessage::Apply => {
                return Some(Merge {
                    columns: self.columns.clone(),
                    separator: self.separator.clone(),
                    name: self.name.trim().to_owned(),
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, MergeMessage> {
        let mut body = column![
            text("Merge columns").size(20),
            text(self.columns.join(", ")).color(tokens.muted_text),
            row![
                text("Separator").width(90),
                text_input("none", &self.separator)
                    .on_input(MergeMessage::Separator)
                    .on_submit(MergeMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text("New column").width(90),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(MergeMessage::Name)
                    .on_submit(MergeMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text("Nulls count as empty text.").color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", MergeMessage::Cancel, MergeMessage::Apply));
        card(body.into(), tokens, 440.0)
    }
}

impl StepDialog for MergeDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Merge(message) = message else {
            return None;
        };
        MergeDialog::update(self, message).map(Step::Merge)
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Merge(MergeMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        MergeDialog::view(self, tokens).map(StepMessage::Merge)
    }

    fn set_error(&mut self, error: String) -> Option<String> {
        MergeDialog::set_error(self, error);
        None
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
