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
            name: merge.map_or_else(|| tr("ST_DLG_MERGE_DEFAULT_NAME"), |m| m.name.clone()),
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
            text(tr("ST_DLG_MERGE_TITLE")).size(20),
            text(self.columns.join(", ")).color(tokens.muted_text),
            row![
                text(tr("ST_DLG_CSV_SEPARATOR")).width(90),
                text_input(&tr("ST_DLG_MERGE_NO_SEPARATOR"), &self.separator)
                    .on_input(MergeMessage::Separator)
                    .on_submit(MergeMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text(tr("ST_DLG_NEW_COLUMN")).width(90),
                text_input(&tr("ST_DLG_COLUMN_NAME_PLACEHOLDER"), &self.name)
                    .id(NAME_ID)
                    .on_input(MergeMessage::Name)
                    .on_submit(MergeMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text(tr("ST_DLG_MERGE_NULLS")).color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).style(iced::widget::text::danger));
        }
        body = body.push(buttons(
            "ST_COMMON_OK",
            MergeMessage::Cancel,
            MergeMessage::Apply,
        ));
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
