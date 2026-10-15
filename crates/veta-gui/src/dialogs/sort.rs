//! Sort by one or more columns.

use super::*;
use iced::widget::column;

/// Sort by one or more columns.
#[derive(Debug, Clone, PartialEq)]
pub struct SortDialog {
    columns: Vec<String>,
    keys: Vec<SortKey>,
}

/// Sort direction, for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        })
    }
}

#[derive(Debug, Clone)]
pub enum SortMessage {
    Column(usize, String),
    Direction(usize, Direction),
    Nulls(usize, Nulls),
    Add,
    Remove(usize),
    Apply,
    Cancel,
}

/// Where nulls go, for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nulls {
    Last,
    First,
}

impl std::fmt::Display for Nulls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Last => "nulls last",
            Self::First => "nulls first",
        })
    }
}

impl SortDialog {
    pub fn new(columns: Vec<String>, sort: &Sort) -> Self {
        Self {
            columns,
            keys: sort.keys.clone(),
        }
    }

    /// Handles a message. Returns the sort to apply on submit.
    pub fn update(&mut self, message: SortMessage) -> Option<Sort> {
        match message {
            SortMessage::Column(i, column) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.column = column;
                }
            }
            SortMessage::Direction(i, direction) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.descending = direction == Direction::Descending;
                }
            }
            SortMessage::Nulls(i, nulls) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.nulls_first = nulls == Nulls::First;
                }
            }
            SortMessage::Add => {
                let column = self
                    .columns
                    .iter()
                    .find(|c| !self.keys.iter().any(|k| &k.column == *c))
                    .or(self.columns.first())
                    .cloned()
                    .unwrap_or_default();
                self.keys.push(SortKey {
                    column,
                    descending: false,
                    nulls_first: false,
                });
            }
            SortMessage::Remove(i) => {
                if self.keys.len() > 1 && i < self.keys.len() {
                    self.keys.remove(i);
                }
            }
            SortMessage::Cancel => {}
            SortMessage::Apply => {
                return Some(Sort {
                    keys: self.keys.clone(),
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, SortMessage> {
        let mut list = column![].spacing(8);
        for (i, key) in self.keys.iter().enumerate() {
            let direction = if key.descending {
                Direction::Descending
            } else {
                Direction::Ascending
            };
            let mut remove = button(text("✕")).style(button::text);
            if self.keys.len() > 1 {
                remove = remove.on_press(SortMessage::Remove(i));
            }
            list = list.push(
                row![
                    text(if i == 0 { "Sort by" } else { "then by" }).width(60),
                    pick_list(self.columns.clone(), Some(key.column.clone()), move |c| {
                        SortMessage::Column(i, c)
                    })
                    .width(Length::Fill),
                    pick_list(
                        [Direction::Ascending, Direction::Descending],
                        Some(direction),
                        move |d| SortMessage::Direction(i, d)
                    )
                    .width(130),
                    pick_list(
                        [Nulls::Last, Nulls::First],
                        Some(if key.nulls_first {
                            Nulls::First
                        } else {
                            Nulls::Last
                        }),
                        move |n| SortMessage::Nulls(i, n)
                    )
                    .width(130),
                    remove,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let body = column![
            text("Sort rows").size(20),
            list,
            button(text("+ Add column"))
                .style(button::text)
                .on_press(SortMessage::Add),
            text("Rows that tie keep their order.").color(tokens.muted_text),
            buttons("OK", SortMessage::Cancel, SortMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 620.0)
    }
}

impl StepDialog for SortDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Sort(message) = message else {
            return None;
        };
        SortDialog::update(self, message).map(Step::Sort)
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Sort(SortMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        SortDialog::view(self, tokens).map(StepMessage::Sort)
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
