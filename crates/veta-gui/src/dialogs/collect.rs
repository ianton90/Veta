//! Rows → Lists: collect columns into lists, one row per group.

use super::*;
use iced::widget::column;

/// Collect columns into lists, grouped by key columns.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectDialog {
    columns: Vec<String>,
    /// The other columns, checked when grouped by.
    keys: Vec<(String, bool)>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CollectMessage {
    ToggleKey(usize, bool),
    Apply,
    Cancel,
}

impl CollectDialog {
    /// Collects `columns`, grouped by the checked `others`.
    pub fn new(columns: Vec<String>, others: Vec<(String, bool)>) -> Self {
        Self {
            columns,
            keys: others,
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// The key columns and the collected ones, on Apply.
    pub fn update(&mut self, message: CollectMessage) -> Option<(Vec<String>, Vec<String>)> {
        self.error = None;
        match message {
            CollectMessage::ToggleKey(i, on) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.1 = on;
                }
            }
            CollectMessage::Cancel => {}
            CollectMessage::Apply => {
                let keys = self
                    .keys
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(name, _)| name.clone())
                    .collect();
                return Some((keys, self.columns.clone()));
            }
        }
        None
    }

    /// The columns left out of the result: neither keys nor collected.
    pub fn dropped(&self) -> Vec<&str> {
        self.keys
            .iter()
            .filter(|(_, on)| !on)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, CollectMessage> {
        let list = column(self.keys.iter().enumerate().map(|(i, (name, on))| {
            checkbox(*on)
                .label(name.as_str())
                .on_toggle(move |on| CollectMessage::ToggleKey(i, on))
                .into()
        }))
        .spacing(6);
        let mut body = column![
            text("Rows → Lists").size(20),
            text(format!("Collect into lists: {}", self.columns.join(", ")))
                .color(tokens.muted_text),
            text("Group by"),
            iced::widget::container(scrollable(list)).max_height(220.0),
            text("One row per group of equal values, ordered by the group columns.")
                .color(tokens.muted_text),
        ]
        .spacing(14);
        let dropped = self.dropped();
        if !dropped.is_empty() {
            body = body.push(
                row![
                    text("Dropped:").font(crate::app::bold()),
                    text(dropped.join(", ")).width(Length::Fill),
                ]
                .spacing(6),
            );
            body = body.push(
                text("Columns neither grouped by nor collected are left out.")
                    .color(tokens.muted_text),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", CollectMessage::Cancel, CollectMessage::Apply));
        card(body.into(), tokens, 440.0)
    }
}

impl StepDialog for CollectDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Collect(message) = message else {
            return None;
        };
        CollectDialog::update(self, message)
            .map(|(keys, columns)| Step::CollectList { keys, columns })
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Collect(CollectMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        CollectDialog::view(self, tokens).map(StepMessage::Collect)
    }

    fn set_error(&mut self, error: String) -> Option<String> {
        CollectDialog::set_error(self, error);
        None
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
