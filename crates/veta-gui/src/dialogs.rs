//! Dialogs for column commands (add, rename, choose columns) and writer
//! settings.

use iced::widget::{Space, button, checkbox, column, pick_list, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use veta_core::arrow::datatypes::{DataType, TimeUnit};
use veta_core::{
    ColumnSettings, Command, Compression, Encoding, FormatVersion, StatisticsLevel, WriterSettings,
};

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

/// Edit the settings used when saving: file-wide settings, defaults for new
/// columns, and per-column settings.
#[derive(Debug, Clone, PartialEq)]
pub struct WriterDialog {
    settings: WriterSettings,
    /// Top-level columns and their types, in order.
    columns: Vec<(String, DataType)>,
    row_group: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WriterMessage {
    Version(FormatVersion),
    RowGroup(String),
    /// `None` edits the defaults for new columns.
    Compression(Option<usize>, Compression),
    Encoding(usize, Encoding),
    Dictionary(Option<usize>, bool),
    Statistics(Option<usize>, StatisticsLevel),
    Bloom(Option<usize>, bool),
    Apply,
    Cancel,
}

impl WriterDialog {
    pub fn new(settings: WriterSettings, columns: Vec<(String, DataType)>) -> Self {
        Self {
            row_group: settings.max_row_group_rows.to_string(),
            settings,
            columns,
            error: None,
        }
    }

    /// Handles a message. Returns the command to run on apply.
    pub fn update(&mut self, message: WriterMessage) -> Option<Command> {
        match message {
            WriterMessage::Version(v) => self.settings.format_version = v,
            WriterMessage::RowGroup(text) => self.row_group = text,
            WriterMessage::Compression(i, c) => self.edit(i, |s| s.compression = c),
            WriterMessage::Encoding(i, e) => self.edit(Some(i), |s| s.encoding = e),
            WriterMessage::Dictionary(i, on) => self.edit(i, |s| s.dictionary = on),
            WriterMessage::Statistics(i, level) => self.edit(i, |s| s.statistics = level),
            WriterMessage::Bloom(i, on) => self.edit(i, |s| s.bloom_filter = on),
            WriterMessage::Cancel => {}
            WriterMessage::Apply => match self.row_group.trim().parse::<usize>() {
                Ok(rows) if rows > 0 => {
                    let mut settings = self.settings.clone();
                    settings.max_row_group_rows = rows;
                    return Some(Command::SetWriterSettings(settings));
                }
                _ => self.error = Some("Rows per row group must be a whole number above 0.".into()),
            },
        }
        None
    }

    fn column_settings(&self, i: Option<usize>) -> ColumnSettings {
        match i.and_then(|i| self.columns.get(i)) {
            Some((name, _)) => *self.settings.column(name),
            None => self.settings.default_column,
        }
    }

    fn edit(&mut self, i: Option<usize>, change: impl FnOnce(&mut ColumnSettings)) {
        let Some((name, _)) = i.and_then(|i| self.columns.get(i)) else {
            change(&mut self.settings.default_column);
            return;
        };
        let mut column = *self.settings.column(name);
        change(&mut column);
        match self.settings.columns.iter_mut().find(|(p, _)| p == name) {
            Some((_, existing)) => *existing = column,
            None => self.settings.columns.push((name.clone(), column)),
        }
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, WriterMessage> {
        const NAME: f32 = 150.0;
        const COMPRESSION: f32 = 150.0;
        const ENCODING: f32 = 200.0;
        const STATISTICS: f32 = 100.0;
        const FLAG: f32 = 90.0;

        let header = row![
            text("Column").width(NAME),
            text("Compression").width(COMPRESSION),
            text("Encoding").width(ENCODING),
            text("Dictionary").width(FLAG),
            text("Statistics").width(STATISTICS),
            text("Bloom filter").width(FLAG),
        ]
        .spacing(8);

        let settings_row = |i: Option<usize>, label: String, encodings: Option<Vec<Encoding>>| {
            let s = self.column_settings(i);
            let encoding: Element<'_, WriterMessage> = match (encodings, i) {
                (Some(choices), Some(i)) => pick_list(choices, Some(s.encoding), move |e| {
                    WriterMessage::Encoding(i, e)
                })
                .width(ENCODING)
                .into(),
                _ => text("plain")
                    .width(ENCODING)
                    .color(tokens.muted_text)
                    .into(),
            };
            row![
                text(label).width(NAME),
                pick_list(
                    Compression::CHOICES,
                    Some(s.compression.without_level()),
                    move |c| { WriterMessage::Compression(i, c) }
                )
                .width(COMPRESSION),
                encoding,
                checkbox(s.dictionary)
                    .on_toggle(move |on| WriterMessage::Dictionary(i, on))
                    .width(FLAG),
                pick_list(StatisticsLevel::ALL, Some(s.statistics), move |l| {
                    WriterMessage::Statistics(i, l)
                })
                .width(STATISTICS),
                checkbox(s.bloom_filter)
                    .on_toggle(move |on| WriterMessage::Bloom(i, on))
                    .width(FLAG),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
        };

        let mut columns = column![header].spacing(6);
        for (i, (name, data_type)) in self.columns.iter().enumerate() {
            columns = columns.push(settings_row(
                Some(i),
                name.clone(),
                Some(Encoding::choices_for(data_type)),
            ));
        }

        let mut body = column![
            text("Writer settings").size(20),
            text("Used when saving. Opened files keep the settings they were written with.")
                .color(tokens.muted_text),
            row![
                text("Format version").width(NAME),
                pick_list(
                    FormatVersion::ALL,
                    Some(self.settings.format_version),
                    WriterMessage::Version
                )
                .width(COMPRESSION),
                Space::new().width(24),
                text("Rows per row group"),
                text_input("1048576", &self.row_group)
                    .on_input(WriterMessage::RowGroup)
                    .width(140),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text("Columns").size(16),
            scrollable(columns).height(Length::Fixed(260.0)),
            settings_row(None, "New columns".into(), None),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons(
            "Apply",
            WriterMessage::Cancel,
            WriterMessage::Apply,
        ));
        card(body.into(), tokens, 860.0)
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
    fn writer_dialog_edits_columns_and_defaults() {
        let mut settings = WriterSettings::default();
        settings
            .columns
            .push(("a".into(), ColumnSettings::default()));
        let mut dialog = WriterDialog::new(
            settings,
            vec![("a".into(), DataType::Int64), ("b".into(), DataType::Utf8)],
        );
        dialog.update(WriterMessage::Compression(Some(0), Compression::Zstd(None)));
        dialog.update(WriterMessage::Encoding(1, Encoding::DeltaByteArray));
        dialog.update(WriterMessage::Dictionary(None, false));
        dialog.update(WriterMessage::Version(FormatVersion::V1));
        dialog.update(WriterMessage::RowGroup("x".into()));
        assert_eq!(dialog.update(WriterMessage::Apply), None);
        assert!(dialog.error.is_some());
        dialog.update(WriterMessage::RowGroup("5000".into()));
        let Some(Command::SetWriterSettings(s)) = dialog.update(WriterMessage::Apply) else {
            panic!("expected settings");
        };
        assert_eq!(s.max_row_group_rows, 5000);
        assert_eq!(s.format_version, FormatVersion::V1);
        assert_eq!(s.column("a").compression, Compression::Zstd(None));
        assert_eq!(s.column("b").encoding, Encoding::DeltaByteArray);
        assert!(!s.default_column.dictionary);
        assert_eq!(s.columns.len(), 2);
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
