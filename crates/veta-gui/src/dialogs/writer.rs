//! Writer settings used when saving.

use super::*;
use iced::widget::column;

/// A compression level choice: `None` is the codec's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level(pub Option<i32>);

impl Level {
    /// Levels offered for `compression` (empty if it takes none).
    pub fn choices(compression: Compression) -> Vec<Level> {
        let mut choices = vec![Level(None)];
        if let Some(range) = compression.level_range() {
            choices.extend(range.map(|l| Level(Some(l))));
        }
        choices
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(level) => write!(f, "{level}"),
            None => f.write_str(&tr("ST_COMMON_DEFAULT")),
        }
    }
}

/// Edit the settings used when saving: file-wide settings, defaults for new
/// columns, and per-column settings.
#[derive(Debug, Clone, PartialEq)]
pub struct WriterDialog {
    settings: WriterSettings,
    /// Leaf columns (dotted Parquet paths) and their types, in order.
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
    Level(Option<usize>, Level),
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
            WriterMessage::Compression(i, c) => self.edit(i, |s| {
                // Keep the level when switching between codecs that take one.
                let level = s
                    .compression
                    .level()
                    .filter(|l| c.level_range().is_some_and(|r| r.contains(l)));
                s.compression = c.with_level(level);
            }),
            WriterMessage::Level(i, level) => {
                self.edit(i, |s| s.compression = s.compression.with_level(level.0));
            }
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
                _ => self.error = Some(tr("ST_SETTINGS_ROW_GROUP_INVALID")),
            },
        }
        None
    }

    pub(super) fn column_settings(&self, i: Option<usize>) -> ColumnSettings {
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
        const NAME: f32 = 160.0;
        const COMPRESSION: f32 = 145.0;
        const LEVEL: f32 = 90.0;
        const ENCODING: f32 = 185.0;
        const STATISTICS: f32 = 90.0;
        const FLAG: f32 = 85.0;

        let header = row![
            text(tr("ST_DLG_WRITER_COLUMN")).width(NAME),
            text(tr("ST_SETTINGS_COMPRESSION")).width(COMPRESSION),
            text(tr("ST_DLG_WRITER_LEVEL")).width(LEVEL),
            text(tr("ST_DLG_WRITER_ENCODING")).width(ENCODING),
            text(tr("ST_SETTINGS_DICTIONARY")).width(FLAG),
            text(tr("ST_SETTINGS_STATISTICS")).width(STATISTICS),
            text(tr("ST_DLG_WRITER_BLOOM")).width(FLAG),
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
                level_picker(s.compression, LEVEL, move |l| WriterMessage::Level(i, l)),
                encoding,
                iced::widget::container(
                    checkbox(s.dictionary).on_toggle(move |on| WriterMessage::Dictionary(i, on))
                )
                .width(FLAG),
                pick_list(StatisticsLevel::ALL, Some(s.statistics), move |l| {
                    WriterMessage::Statistics(i, l)
                })
                .width(STATISTICS),
                iced::widget::container(
                    checkbox(s.bloom_filter).on_toggle(move |on| WriterMessage::Bloom(i, on))
                )
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
            text(tr("ST_DLG_WRITER_TITLE")).size(20),
            text(tr("ST_DLG_WRITER_NOTE")).color(tokens.muted_text),
            row![
                text(tr("ST_SETTINGS_FORMAT_VERSION")).width(NAME),
                pick_list(
                    FormatVersion::ALL,
                    Some(self.settings.format_version),
                    WriterMessage::Version
                )
                .width(COMPRESSION),
                Space::new().width(24),
                text(tr("ST_SETTINGS_ROW_GROUP")),
                text_input("1048576", &self.row_group)
                    .on_input(WriterMessage::RowGroup)
                    .width(140),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text(tr("ST_DLG_WRITER_COLUMNS")).size(16),
            scrollable(columns.padding(iced::Padding::ZERO.right(16))).height(Length::Fixed(260.0)),
            settings_row(None, tr("ST_DLG_WRITER_NEW_COLUMNS"), None),
            text(tr("ST_DLG_WRITER_NESTED_NOTE"))
                .size(13)
                .color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).style(iced::widget::text::danger));
        }
        body = body.push(buttons(
            "ST_COMMON_APPLY",
            WriterMessage::Cancel,
            WriterMessage::Apply,
        ));
        card(body.into(), tokens, 940.0)
    }
}

/// A level pick list for `compression`, or a dash for codecs without levels.
pub fn level_picker<'a, M: Clone + 'a>(
    compression: Compression,
    width: f32,
    on_pick: impl Fn(Level) -> M + 'a,
) -> Element<'a, M> {
    if compression.level_range().is_none() {
        return iced::widget::container(text("—"))
            .width(width)
            .align_y(Alignment::Center)
            .into();
    }
    pick_list(
        Level::choices(compression),
        Some(Level(compression.level())),
        on_pick,
    )
    .width(width)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn compression_levels() {
        let mut dialog = WriterDialog::new(
            WriterSettings::default(),
            vec![("a".into(), DataType::Int64)],
        );
        dialog.update(WriterMessage::Compression(Some(0), Compression::Zstd(None)));
        dialog.update(WriterMessage::Level(Some(0), Level(Some(7))));
        assert_eq!(
            dialog.column_settings(Some(0)).compression,
            Compression::Zstd(Some(7))
        );
        // Gzip takes 0-10: the level carries over; Brotli then keeps it too.
        dialog.update(WriterMessage::Compression(Some(0), Compression::Gzip(None)));
        assert_eq!(
            dialog.column_settings(Some(0)).compression,
            Compression::Gzip(Some(7))
        );
        dialog.update(WriterMessage::Compression(Some(0), Compression::Snappy));
        assert_eq!(
            dialog.column_settings(Some(0)).compression,
            Compression::Snappy
        );
        assert_eq!(Level::choices(Compression::Zstd(None)).len(), 23);
        assert_eq!(Level::choices(Compression::Snappy), vec![Level(None)]);
    }
}
