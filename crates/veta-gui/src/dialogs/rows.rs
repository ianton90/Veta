//! Insert, remove or keep a number of rows.

use super::*;
use iced::widget::column;

/// Where rows are inserted, deleted or kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowPlace {
    Top,
    Bottom,
    At,
}

impl RowPlace {
    const ALL: [RowPlace; 3] = [Self::Top, Self::Bottom, Self::At];
}

impl std::fmt::Display for RowPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&tr(match self {
            Self::Top => "ST_DLG_ROWS_TOP",
            Self::Bottom => "ST_DLG_ROWS_BOTTOM",
            Self::At => "ST_DLG_ROWS_AT",
        }))
    }
}

pub use veta_core::rows::RowAction;

/// Insert, delete (remove) or keep a number of rows at the top, the bottom
/// or a row.
#[derive(Debug, Clone, PartialEq)]
pub struct RowsDialog {
    action: RowAction,
    title: &'static str,
    num_rows: usize,
    count: String,
    place: RowPlace,
    /// Row number (from 1) for [`RowPlace::At`].
    at: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RowsMessage {
    Count(String),
    Place(RowPlace),
    At(String),
    Apply,
    Cancel,
}

impl RowsDialog {
    /// For a document of `num_rows` rows; `selected` (rows) sets the
    /// starting count and position. `title` names the command.
    pub fn new(
        action: RowAction,
        title: &'static str,
        num_rows: usize,
        selected: Option<std::ops::Range<usize>>,
    ) -> Self {
        let (count, place, at) = match selected {
            Some(rows) => (rows.len(), RowPlace::At, rows.start + 1),
            None if action == RowAction::Insert => (1, RowPlace::Bottom, num_rows + 1),
            None => (1, RowPlace::Top, 1),
        };
        Self {
            action,
            title,
            num_rows,
            count: count.to_string(),
            place,
            at: at.to_string(),
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// The command for the dialog's input, checked.
    fn command(&self) -> Result<Command, String> {
        use veta_core::rows::{RowPlace as Place, rows_command};
        let count: usize = self
            .count
            .trim()
            .parse()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| tr("ST_DLG_ROWS_COUNT_INVALID"))?;
        let place = match self.place {
            RowPlace::Top => Place::Top,
            RowPlace::Bottom => Place::Bottom,
            RowPlace::At => {
                let at: usize = self
                    .at
                    .trim()
                    .parse()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or_else(|| tr("ST_DLG_ROWS_START_AT_1"))?;
                Place::At(at - 1)
            }
        };
        rows_command(self.action, place, count, self.num_rows)
            .map_err(|e| sentence(&error_text(&e)))
    }

    /// The command to run, on Apply.
    pub fn update(&mut self, message: RowsMessage) -> Option<Command> {
        self.error = None;
        match message {
            RowsMessage::Count(count) => self.count = count,
            RowsMessage::Place(place) => self.place = place,
            RowsMessage::At(at) => self.at = at,
            RowsMessage::Cancel => {}
            RowsMessage::Apply => match self.command() {
                Ok(command) => return Some(command),
                Err(e) => self.error = Some(e),
            },
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, RowsMessage> {
        let action = match self.action {
            RowAction::Insert => "ST_DLG_ROWS_INSERT",
            RowAction::Delete => "ST_DLG_ROWS_REMOVE",
            RowAction::Keep => "ST_DLG_ROWS_KEEP",
        };
        let mut body = column![
            text(tr(self.title)).size(20),
            row![
                text(tr("ST_DLG_ROWS_ROWS")).width(90),
                text_input("1", &self.count)
                    .id(NAME_ID)
                    .on_input(RowsMessage::Count)
                    .on_submit(RowsMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text(tr("ST_DLG_ROWS_WHERE")).width(90),
                pick_list(RowPlace::ALL, Some(self.place), RowsMessage::Place).width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(14);
        if self.place == RowPlace::At {
            body = body.push(
                row![
                    text(tr("ST_DLG_ROWS_ROW_NUMBER")).width(90),
                    text_input("1", &self.at)
                        .on_input(RowsMessage::At)
                        .on_submit(RowsMessage::Apply),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        }
        let note = match self.action {
            RowAction::Insert => {
                tr_with("ST_DLG_ROWS_INSERT_NOTE", &[("end", &(self.num_rows + 1))])
            }
            RowAction::Delete => tr_with("ST_DLG_ROWS_REMOVE_NOTE", &[("n", &self.num_rows)]),
            RowAction::Keep => tr_with("ST_DLG_ROWS_KEEP_NOTE", &[("n", &self.num_rows)]),
        };
        body = body.push(text(note).color(tokens.muted_text));
        if let Some(error) = &self.error {
            body = body.push(text(error).style(iced::widget::text::danger));
        }
        body = body.push(buttons(action, RowsMessage::Cancel, RowsMessage::Apply));
        card(body.into(), tokens, 420.0)
    }
}

/// `message` as a sentence: capitalised, ending with a full stop.
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    out.push_str(chars.as_str());
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}
