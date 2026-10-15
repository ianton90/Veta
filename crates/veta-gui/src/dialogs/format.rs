//! Trim or change the case of text columns.

use super::*;
use iced::widget::column;

/// Trim or change the case of text columns.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatDialog {
    columns: Vec<String>,
    op: TextOp,
}

#[derive(Debug, Clone)]
pub enum FormatMessage {
    Op(TextOp),
    Apply,
    Cancel,
}

impl FormatDialog {
    pub fn new(columns: Vec<String>, op: TextOp) -> Self {
        Self { columns, op }
    }

    /// Returns the columns and operation on submit.
    pub fn update(&mut self, message: FormatMessage) -> Option<(Vec<String>, TextOp)> {
        match message {
            FormatMessage::Op(op) => self.op = op,
            FormatMessage::Cancel => {}
            FormatMessage::Apply => return Some((self.columns.clone(), self.op)),
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, FormatMessage> {
        let body = column![
            text("Format text").size(20),
            text(self.columns.join(", ")).color(tokens.muted_text),
            pick_list(TextOp::ALL, Some(self.op), FormatMessage::Op).width(Length::Fill),
            buttons("OK", FormatMessage::Cancel, FormatMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 400.0)
    }
}

impl StepDialog for FormatDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Format(message) = message else {
            return None;
        };
        FormatDialog::update(self, message).map(|(columns, op)| Step::FormatText { columns, op })
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Format(FormatMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        FormatDialog::view(self, tokens).map(StepMessage::Format)
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
