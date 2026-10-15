//! Split a column into several.

use super::*;
use iced::widget::column;

/// How to split a column, for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitMode {
    Delimiter,
    Positions,
}

impl std::fmt::Display for SplitMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Delimiter => "by delimiter",
            Self::Positions => "at character positions",
        })
    }
}

/// Split a column into several.
#[derive(Debug, Clone)]
pub struct SplitDialog {
    column: String,
    sample: ArrayRef,
    mode: SplitMode,
    delimiter: String,
    parts: String,
    positions: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SplitMessage {
    Mode(SplitMode),
    Delimiter(String),
    Parts(String),
    Positions(String),
    Apply,
    Cancel,
}

impl SplitDialog {
    /// `sample` (first values of the column) suggests how many parts.
    pub fn new(column: String, sample: ArrayRef, split: Option<&Split>) -> Self {
        let mut dialog = Self {
            column,
            sample,
            mode: SplitMode::Delimiter,
            delimiter: ",".into(),
            parts: String::new(),
            positions: String::new(),
            error: None,
        };
        match split.map(|s| &s.by) {
            Some(SplitBy::Delimiter { delimiter, parts }) => {
                dialog.delimiter = delimiter.clone();
                dialog.parts = parts.to_string();
            }
            Some(SplitBy::Positions(positions)) => {
                dialog.mode = SplitMode::Positions;
                let list: Vec<String> = positions.iter().map(ToString::to_string).collect();
                dialog.positions = list.join(", ");
                dialog.suggest_parts();
            }
            None => dialog.suggest_parts(),
        }
        dialog
    }

    fn suggest_parts(&mut self) {
        self.parts = count_parts(&self.sample, &self.delimiter)
            .max(2)
            .to_string();
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: SplitMessage) -> Option<Split> {
        self.error = None;
        match message {
            SplitMessage::Mode(mode) => self.mode = mode,
            SplitMessage::Delimiter(d) => {
                self.delimiter = d;
                self.suggest_parts();
            }
            SplitMessage::Parts(p) => self.parts = p,
            SplitMessage::Positions(p) => self.positions = p,
            SplitMessage::Cancel => {}
            SplitMessage::Apply => {
                let by = match self.mode {
                    SplitMode::Delimiter => match self.parts.trim().parse::<usize>() {
                        Ok(parts) => SplitBy::Delimiter {
                            delimiter: self.delimiter.clone(),
                            parts,
                        },
                        Err(_) => {
                            self.error = Some("Columns must be a whole number.".into());
                            return None;
                        }
                    },
                    SplitMode::Positions => {
                        let parsed: Result<Vec<usize>, _> = self
                            .positions
                            .split(',')
                            .filter(|p| !p.trim().is_empty())
                            .map(|p| p.trim().parse::<usize>())
                            .collect();
                        match parsed {
                            Ok(positions) => SplitBy::Positions(positions),
                            Err(_) => {
                                self.error =
                                    Some("Positions must be whole numbers, like 2, 5.".into());
                                return None;
                            }
                        }
                    }
                };
                return Some(Split {
                    column: self.column.clone(),
                    by,
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, SplitMessage> {
        let labelled = |label: &'static str, control: Element<'static, SplitMessage>| {
            row![text(label).width(90), control]
                .spacing(12)
                .align_y(Alignment::Center)
        };
        let mut body = column![
            text(format!("Split {}", self.column)).size(20),
            labelled(
                "Split",
                pick_list(
                    [SplitMode::Delimiter, SplitMode::Positions],
                    Some(self.mode),
                    SplitMessage::Mode
                )
                .width(Length::Fill)
                .into()
            ),
        ]
        .spacing(14);
        match self.mode {
            SplitMode::Delimiter => {
                body = body.push(labelled(
                    "Delimiter",
                    text_input(",", &self.delimiter)
                        .id(NAME_ID)
                        .on_input(SplitMessage::Delimiter)
                        .on_submit(SplitMessage::Apply)
                        .into(),
                ));
                body = body.push(labelled(
                    "Columns",
                    text_input("2", &self.parts)
                        .on_input(SplitMessage::Parts)
                        .on_submit(SplitMessage::Apply)
                        .width(80)
                        .into(),
                ));
                body = body.push(
                    text("The last column keeps any text left over.").color(tokens.muted_text),
                );
            }
            SplitMode::Positions => {
                body = body.push(labelled(
                    "Positions",
                    text_input("e.g. 2, 5", &self.positions)
                        .id(NAME_ID)
                        .on_input(SplitMessage::Positions)
                        .on_submit(SplitMessage::Apply)
                        .into(),
                ));
                body = body.push(
                    text("Character counts from the start where each new column begins.")
                        .color(tokens.muted_text),
                );
            }
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", SplitMessage::Cancel, SplitMessage::Apply));
        card(body.into(), tokens, 440.0)
    }
}
