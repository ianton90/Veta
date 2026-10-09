//! Small dialogs for column commands: add, rename, choose columns.

use iced::widget::{Space, button, checkbox, column, pick_list, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use veta_core::Command;
use veta_core::arrow::datatypes::{DataType, TimeUnit};

use crate::settings::card;
use crate::theme::Tokens;

/// Id of the name input, focused when a column dialog opens.
pub const NAME_ID: iced::widget::Id = iced::widget::Id::new("column-name");

/// Types offered when adding a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeChoice {
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
    Timestamp,
}

impl TypeChoice {
    const ALL: [TypeChoice; 6] = [
        Self::Text,
        Self::Integer,
        Self::Decimal,
        Self::Boolean,
        Self::Date,
        Self::Timestamp,
    ];

    fn data_type(self) -> DataType {
        match self {
            Self::Text => DataType::Utf8,
            Self::Integer => DataType::Int64,
            Self::Decimal => DataType::Float64,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
        }
    }
}

impl std::fmt::Display for TypeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text => "Text (string)",
            Self::Integer => "Whole number (int64)",
            Self::Decimal => "Decimal number (float64)",
            Self::Boolean => "True/false (bool)",
            Self::Date => "Date",
            Self::Timestamp => "Date and time (timestamp)",
        })
    }
}

/// Add or rename a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDialog {
    pub mode: ColumnMode,
    name: String,
    choice: TypeChoice,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnMode {
    Add { at: usize },
    Rename { from: String },
}

#[derive(Debug, Clone)]
pub enum ColumnMessage {
    Name(String),
    Type(TypeChoice),
    Submit,
    Cancel,
}

impl ColumnDialog {
    pub fn add(at: usize) -> Self {
        Self {
            mode: ColumnMode::Add { at },
            name: String::new(),
            choice: TypeChoice::Text,
        }
    }

    pub fn rename(from: String) -> Self {
        Self {
            name: from.clone(),
            mode: ColumnMode::Rename { from },
            choice: TypeChoice::Text,
        }
    }

    /// Handles a message. Returns the command to run on submit.
    pub fn update(&mut self, message: ColumnMessage) -> Option<Command> {
        match message {
            ColumnMessage::Name(name) => self.name = name,
            ColumnMessage::Type(choice) => self.choice = choice,
            ColumnMessage::Cancel => {}
            ColumnMessage::Submit => {
                let name = self.name.trim().to_owned();
                return Some(match &self.mode {
                    ColumnMode::Add { at } => Command::AddColumn {
                        name,
                        data_type: self.choice.data_type(),
                        at: *at,
                    },
                    ColumnMode::Rename { from } => Command::RenameColumn {
                        from: from.clone(),
                        to: name,
                    },
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ColumnMessage> {
        let (title, action) = match &self.mode {
            ColumnMode::Add { .. } => ("Add column", "Add"),
            ColumnMode::Rename { .. } => ("Rename column", "Rename"),
        };
        let mut body = column![
            text(title).size(20),
            row![
                text("Name").width(70),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(ColumnMessage::Name)
                    .on_submit(ColumnMessage::Submit),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(16);
        if let ColumnMode::Add { .. } = self.mode {
            body = body.push(
                row![
                    text("Type").width(70),
                    pick_list(TypeChoice::ALL, Some(self.choice), ColumnMessage::Type)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
            body = body.push(text("The new column starts empty (null).").color(tokens.muted_text));
        }
        body = body.push(buttons(
            action,
            ColumnMessage::Cancel,
            ColumnMessage::Submit,
        ));
        card(body.into(), tokens, 420.0)
    }
}

/// Pick the columns to keep (Power Query's "Choose Columns").
#[derive(Debug, Clone, PartialEq)]
pub struct ChooseColumns {
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
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            columns: names.into_iter().map(|n| (n, true)).collect(),
        }
    }

    /// Handles a message. Returns the command to run on apply, if any
    /// column was unchecked.
    pub fn update(&mut self, message: ChooseMessage) -> Option<Option<Command>> {
        match message {
            ChooseMessage::Toggle(i, keep) => {
                if let Some(column) = self.columns.get_mut(i) {
                    column.1 = keep;
                }
            }
            ChooseMessage::All(keep) => {
                for column in &mut self.columns {
                    column.1 = keep;
                }
            }
            ChooseMessage::Cancel => {}
            ChooseMessage::Apply => {
                let names: Vec<String> = self
                    .columns
                    .iter()
                    .filter(|(_, keep)| !keep)
                    .map(|(name, _)| name.clone())
                    .collect();
                return Some((!names.is_empty()).then_some(Command::RemoveColumns { names }));
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ChooseMessage> {
        let all = self.columns.iter().all(|(_, keep)| *keep);
        let list = column(self.columns.iter().enumerate().map(|(i, (name, keep))| {
            checkbox(*keep)
                .label(name.as_str())
                .on_toggle(move |keep| ChooseMessage::Toggle(i, keep))
                .into()
        }))
        .spacing(6);
        let kept = self.columns.iter().filter(|(_, keep)| *keep).count();
        let body = column![
            text("Choose columns").size(20),
            checkbox(all)
                .label("Select all")
                .on_toggle(ChooseMessage::All),
            scrollable(list).height(Length::Fixed(320.0)),
            text(format!("{kept} of {} columns kept", self.columns.len())).color(tokens.muted_text),
            buttons("OK", ChooseMessage::Cancel, ChooseMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 420.0)
    }
}

fn buttons<'a, M: Clone + 'a>(action: &'a str, cancel: M, submit: M) -> Element<'a, M> {
    row![
        Space::new().width(Length::Fill),
        button(text("Cancel"))
            .style(button::secondary)
            .on_press(cancel),
        button(text(action)).on_press(submit),
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_rename_build_commands() {
        let mut add = ColumnDialog::add(2);
        add.update(ColumnMessage::Name(" total ".into()));
        add.update(ColumnMessage::Type(TypeChoice::Decimal));
        assert_eq!(
            add.update(ColumnMessage::Submit),
            Some(Command::AddColumn {
                name: "total".into(),
                data_type: DataType::Float64,
                at: 2
            })
        );
        let mut rename = ColumnDialog::rename("a".into());
        rename.update(ColumnMessage::Name("b".into()));
        assert_eq!(
            rename.update(ColumnMessage::Submit),
            Some(Command::RenameColumn {
                from: "a".into(),
                to: "b".into()
            })
        );
    }

    #[test]
    fn choose_columns_removes_unchecked() {
        let mut choose = ChooseColumns::new(["a", "b", "c"].map(String::from));
        assert_eq!(choose.update(ChooseMessage::Apply), Some(None));
        choose.update(ChooseMessage::Toggle(1, false));
        choose.update(ChooseMessage::All(false));
        choose.update(ChooseMessage::Toggle(0, true));
        assert_eq!(
            choose.update(ChooseMessage::Apply),
            Some(Some(Command::RemoveColumns {
                names: vec!["b".into(), "c".into()]
            }))
        );
    }
}
