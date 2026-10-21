//! Pick columns to keep or to remove.

use super::*;
use iced::widget::column;

/// Pick columns to keep or to remove.
#[derive(Debug, Clone, PartialEq)]
pub struct ChooseColumns {
    /// Keep the checked columns (otherwise remove them).
    keep: bool,
    columns: Vec<(String, bool)>,
}

#[derive(Debug, Clone)]
pub enum ChooseMessage {
    Toggle(usize, bool),
    All(bool),
    Apply,
    Cancel,
}

impl ChooseColumns {
    /// `checked` columns start checked; with none, all start checked when
    /// keeping and unchecked when removing.
    pub fn new(keep: bool, names: impl IntoIterator<Item = String>, checked: &[String]) -> Self {
        let columns = names
            .into_iter()
            .map(|n| {
                let on = if checked.is_empty() {
                    keep
                } else {
                    checked.contains(&n)
                };
                (n, on)
            })
            .collect();
        Self { keep, columns }
    }

    /// Handles a message. Returns the command to run on apply, if it changes
    /// anything.
    pub fn update(&mut self, message: ChooseMessage) -> Option<Option<Command>> {
        match message {
            ChooseMessage::Toggle(i, on) => {
                if let Some(column) = self.columns.get_mut(i) {
                    column.1 = on;
                }
            }
            ChooseMessage::All(on) => {
                for column in &mut self.columns {
                    column.1 = on;
                }
            }
            ChooseMessage::Cancel => {}
            ChooseMessage::Apply => {
                let names: Vec<String> = self
                    .columns
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(name, _)| name.clone())
                    .collect();
                let command = if self.keep {
                    (names.len() < self.columns.len()).then_some(Command::KeepColumns { names })
                } else {
                    (!names.is_empty()).then_some(Command::RemoveColumns { names })
                };
                return Some(command);
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ChooseMessage> {
        let all = self.columns.iter().all(|(_, on)| *on);
        let list = column(self.columns.iter().enumerate().map(|(i, (name, on))| {
            checkbox(*on)
                .label(name.as_str())
                .on_toggle(move |on| ChooseMessage::Toggle(i, on))
                .into()
        }))
        .spacing(6);
        let checked = self.columns.iter().filter(|(_, on)| *on).count();
        let (title, summary) = if self.keep {
            (
                tr("ST_DLG_CHOOSE_KEEP_TITLE"),
                tr_with(
                    "ST_DLG_CHOOSE_KEPT",
                    &[("n", &checked), ("total", &self.columns.len())],
                ),
            )
        } else {
            (
                tr("ST_DLG_CHOOSE_REMOVE_TITLE"),
                tr_with(
                    "ST_DLG_CHOOSE_REMOVED",
                    &[("n", &checked), ("total", &self.columns.len())],
                ),
            )
        };
        let body = column![
            text(title).size(20),
            checkbox(all)
                .label(tr("ST_DLG_CHOOSE_ALL"))
                .on_toggle(ChooseMessage::All),
            scrollable(list).height(Length::Fixed(320.0)),
            text(summary).color(tokens.muted_text),
            buttons("ST_COMMON_OK", ChooseMessage::Cancel, ChooseMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 420.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_columns_removes_unchecked() {
        let names = ["a", "b", "c"].map(String::from);
        let mut keep = ChooseColumns::new(true, names.clone(), &[]);
        assert_eq!(keep.update(ChooseMessage::Apply), Some(None));
        keep.update(ChooseMessage::All(false));
        keep.update(ChooseMessage::Toggle(0, true));
        assert_eq!(
            keep.update(ChooseMessage::Apply),
            Some(Some(Command::KeepColumns {
                names: vec!["a".into()]
            }))
        );
        let mut remove = ChooseColumns::new(false, names, &["b".into()]);
        remove.update(ChooseMessage::Toggle(2, true));
        assert_eq!(
            remove.update(ChooseMessage::Apply),
            Some(Some(Command::RemoveColumns {
                names: vec!["b".into(), "c".into()]
            }))
        );
    }
}
