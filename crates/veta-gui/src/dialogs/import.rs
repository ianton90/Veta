//! Import a CSV file.

use super::*;
use iced::widget::column;

/// Field separators offered for CSV files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
    Pipe,
    /// Typed by the user.
    Other,
}

impl Delimiter {
    pub const ALL: [Delimiter; 5] = [
        Self::Comma,
        Self::Semicolon,
        Self::Tab,
        Self::Pipe,
        Self::Other,
    ];

    fn byte(self) -> Option<u8> {
        Some(match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
            Self::Pipe => b'|',
            Self::Other => return None,
        })
    }
}

/// The separator byte: a preset, or the single ASCII character typed.
pub(super) fn separator(delimiter: Delimiter, other: &str) -> Result<u8, String> {
    if let Some(b) = delimiter.byte() {
        return Ok(b);
    }
    match other.as_bytes() {
        [b] if b.is_ascii() && !b.is_ascii_alphanumeric() && *b != b'\n' => Ok(*b),
        _ => Err(tr("ST_DLG_CSV_SEPARATOR_INVALID")),
    }
}

/// Quote characters (text qualifiers) for CSV files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    Double,
    Single,
    None,
}

impl Quote {
    pub const ALL: [Quote; 3] = [Self::Double, Self::Single, Self::None];

    pub(super) fn byte(self) -> Option<u8> {
        match self {
            Self::Double => Some(b'"'),
            Self::Single => Some(b'\''),
            Self::None => None,
        }
    }
}

impl std::fmt::Display for Quote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&tr(match self {
            Self::Double => "ST_DLG_CSV_QUOTE_DOUBLE",
            Self::Single => "ST_DLG_CSV_QUOTE_SINGLE",
            Self::None => "ST_DLG_CSV_QUOTE_NONE",
        }))
    }
}

impl std::fmt::Display for Delimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&tr(match self {
            Self::Comma => "ST_DLG_CSV_DELIMITER_COMMA",
            Self::Semicolon => "ST_DLG_CSV_DELIMITER_SEMICOLON",
            Self::Tab => "ST_DLG_CSV_DELIMITER_TAB",
            Self::Pipe => "ST_DLG_CSV_DELIMITER_PIPE",
            Self::Other => "ST_DLG_CSV_DELIMITER_OTHER",
        }))
    }
}

/// Decimal separators, for the pick lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalMark {
    Dot,
    Comma,
}

impl DecimalMark {
    pub(super) fn char(self) -> char {
        match self {
            Self::Dot => '.',
            Self::Comma => ',',
        }
    }
}

impl std::fmt::Display for DecimalMark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&tr(match self {
            Self::Dot => "ST_DLG_CAST_SEPARATOR_DOT",
            Self::Comma => "ST_DLG_CAST_SEPARATOR_COMMA",
        }))
    }
}

/// Column types offered when importing CSV.
const IMPORT_TYPES: [CastTarget; 9] = [
    CastTarget::Text,
    CastTarget::Int64,
    CastTarget::Int32,
    CastTarget::Float64,
    CastTarget::Float32,
    CastTarget::Boolean,
    CastTarget::Date,
    CastTarget::Timestamp,
    CastTarget::Time,
];

/// Rows shown in the import preview.
const PREVIEW_ROWS: usize = 12;

/// Import a CSV file: options, a preview and column types.
#[derive(Debug, Clone)]
pub struct ImportDialog {
    pub path: std::path::PathBuf,
    delimiter: Delimiter,
    /// The separator typed when `delimiter` is `Other`.
    other: String,
    quote: Quote,
    header: bool,
    encoding: TextEncoding,
    decimal: DecimalMark,
    /// Column types chosen (initially the guessed ones).
    types: Vec<DataType>,
    /// Format of date and timestamp columns; empty reads ISO 8601.
    date_format: String,
    on_error: OnError,
    preview: Result<CsvPreview, String>,
}

#[derive(Debug, Clone)]
pub enum ImportMessage {
    Delimiter(Delimiter),
    Other(String),
    Quote(Quote),
    Header(bool),
    Encoding(TextEncoding),
    Decimal(DecimalMark),
    Type(usize, CastTarget),
    DateFormat(String),
    OnError(OnError),
    Import,
    Cancel,
}

impl ImportDialog {
    pub fn new(path: std::path::PathBuf) -> Self {
        let tab = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
        let mut dialog = Self {
            path,
            delimiter: if tab {
                Delimiter::Tab
            } else {
                Delimiter::Comma
            },
            other: String::new(),
            quote: Quote::Double,
            header: true,
            encoding: TextEncoding::Utf8,
            decimal: DecimalMark::Dot,
            types: Vec::new(),
            date_format: String::new(),
            on_error: OnError::ErrorColumn,
            preview: Err(String::new()),
        };
        if !tab {
            dialog.guess_delimiter();
        }
        dialog.refresh(true);
        dialog
    }

    /// Picks the separator that splits the first line into most fields.
    fn guess_delimiter(&mut self) {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return;
        };
        let mut head = vec![0u8; 4096];
        let n = std::io::Read::read(&mut file, &mut head).unwrap_or(0);
        let first = head[..n].split(|&b| b == b'\n').next().unwrap_or_default();
        if let Some(best) = Delimiter::ALL
            .into_iter()
            .filter_map(|d| Some((d, d.byte()?)))
            .max_by_key(|(_, b)| first.iter().filter(|&&c| c == *b).count())
            .filter(|(_, b)| first.contains(b))
            .map(|(d, _)| d)
        {
            self.delimiter = best;
            if best == Delimiter::Semicolon {
                self.decimal = DecimalMark::Comma;
            }
        }
    }

    pub fn options(&self) -> CsvOptions {
        CsvOptions {
            delimiter: separator(self.delimiter, &self.other).unwrap_or(b','),
            quote: self.quote.byte(),
            header: self.header,
            encoding: self.encoding,
            decimal: self.decimal.char(),
            types: (!self.types.is_empty()).then(|| self.types.clone()),
            date_format: Some(self.date_format.trim().to_owned()).filter(|f| !f.is_empty()),
            on_error: self.on_error,
        }
    }

    fn has_dates(&self) -> bool {
        self.types
            .iter()
            .any(|t| matches!(t, DataType::Date32 | DataType::Timestamp(..)))
    }

    /// Re-reads the preview; `guess` drops chosen types and guesses again.
    fn refresh(&mut self, guess: bool) {
        if guess {
            self.types.clear();
        }
        if let Err(e) = separator(self.delimiter, &self.other) {
            self.preview = Err(e);
            return;
        }
        self.preview =
            preview_csv(&self.path, &self.options(), PREVIEW_ROWS).map_err(|e| e.to_string());
        if let Ok(preview) = &self.preview {
            self.types = preview
                .schema
                .fields()
                .iter()
                .map(|f| f.data_type().clone())
                .collect();
        }
    }

    /// Handles a message. Returns the options to import with on submit.
    pub fn update(&mut self, message: ImportMessage) -> Option<CsvOptions> {
        match message {
            ImportMessage::Delimiter(d) => {
                self.delimiter = d;
                self.refresh(true);
            }
            ImportMessage::Other(text) => {
                self.other = text;
                self.refresh(true);
            }
            ImportMessage::Quote(q) => {
                self.quote = q;
                self.refresh(true);
            }
            ImportMessage::Header(on) => {
                self.header = on;
                self.refresh(true);
            }
            ImportMessage::Encoding(e) => {
                self.encoding = e;
                self.refresh(false);
            }
            ImportMessage::Decimal(d) => {
                self.decimal = d;
                self.refresh(true);
            }
            ImportMessage::Type(i, target) => {
                if let (Some(t), Some(to)) = (self.types.get_mut(i), target.data_type()) {
                    *t = to;
                }
                self.refresh(false);
            }
            ImportMessage::DateFormat(format) => {
                self.date_format = format;
                self.refresh(false);
            }
            ImportMessage::OnError(e) => self.on_error = e,
            ImportMessage::Cancel => {}
            ImportMessage::Import => {
                return self.preview.is_ok().then(|| self.options());
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ImportMessage> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let options = row![
            column![
                text(tr("ST_DLG_CSV_SEPARATOR")).size(13),
                row![
                    pick_list(
                        Delimiter::ALL,
                        Some(self.delimiter),
                        ImportMessage::Delimiter
                    )
                    .width(160),
                ]
                .extend((self.delimiter == Delimiter::Other).then(|| {
                    text_input("", &self.other)
                        .on_input(ImportMessage::Other)
                        .width(36)
                        .into()
                }))
                .spacing(4),
            ]
            .spacing(4),
            column![
                text(tr("ST_DLG_CSV_QUOTE")).size(13),
                pick_list(Quote::ALL, Some(self.quote), ImportMessage::Quote).width(130),
            ]
            .spacing(4),
            column![
                text(tr("ST_DLG_CSV_DECIMAL")).size(13),
                pick_list(
                    [DecimalMark::Dot, DecimalMark::Comma],
                    Some(self.decimal),
                    ImportMessage::Decimal
                )
                .width(140),
            ]
            .spacing(4),
            column![
                text(tr("ST_DLG_WRITER_ENCODING")).size(13),
                pick_list(
                    TextEncoding::ALL.map(Labeled),
                    Some(Labeled(self.encoding)),
                    |e| ImportMessage::Encoding(e.0)
                )
                .width(150),
            ]
            .spacing(4),
            column![
                text(" ").size(13),
                checkbox(self.header)
                    .label(tr("ST_DLG_IMPORT_HEADER"))
                    .on_toggle(ImportMessage::Header),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .align_y(Alignment::End);
        let preview: Element<'_, ImportMessage> = match &self.preview {
            Err(e) => text(if e.is_empty() {
                tr("ST_DLG_IMPORT_READING")
            } else {
                e.clone()
            })
            .color(iced::Color::from_rgb8(0xd1, 0x43, 0x43))
            .into(),
            Ok(preview) => {
                const WIDTH: f32 = 150.0;
                let cells = veta_core::display::format_batch(&preview.rows).unwrap_or_default();
                let mut header = row![].spacing(6);
                for (i, field) in preview.schema.fields().iter().enumerate() {
                    let choice = IMPORT_TYPES
                        .into_iter()
                        .find(|t| t.data_type().as_ref() == Some(field.data_type()));
                    let picker: Element<'_, ImportMessage> = match choice {
                        Some(c) => {
                            pick_list(IMPORT_TYPES, Some(c), move |t| ImportMessage::Type(i, t))
                                .text_size(12)
                                .width(WIDTH)
                                .into()
                        }
                        None => pick_list(IMPORT_TYPES, None::<CastTarget>, move |t| {
                            ImportMessage::Type(i, t)
                        })
                        .placeholder(veta_core::display::type_name(field.data_type()))
                        .text_size(12)
                        .width(WIDTH)
                        .into(),
                    };
                    let failed = preview.failures.get(i).copied().unwrap_or(0);
                    let note = if failed > 0 {
                        text(tr_with("ST_DLG_IMPORT_DONT_FIT", &[("n", &failed)]))
                            .size(11)
                            .color(iced::Color::from_rgb8(0xd1, 0x43, 0x43))
                    } else {
                        text(" ").size(11)
                    };
                    header = header.push(
                        column![
                            text(field.name().clone()).size(13).width(WIDTH),
                            picker,
                            note
                        ]
                        .spacing(2),
                    );
                }
                let mut grid = column![header].spacing(3);
                for line in cells {
                    let mut r = row![].spacing(6);
                    for value in line {
                        let (value, color) = match value {
                            Some(v) => (v, tokens.text),
                            None => ("null".into(), tokens.muted_text),
                        };
                        r = r.push(text(value).size(12).color(color).width(WIDTH));
                    }
                    grid = grid.push(r);
                }
                scrollable(grid)
                    .direction(scrollable::Direction::Both {
                        vertical: scrollable::Scrollbar::default(),
                        horizontal: scrollable::Scrollbar::default(),
                    })
                    .height(Length::Fixed(300.0))
                    .into()
            }
        };
        let date_format: Option<Element<'_, ImportMessage>> = self.has_dates().then(|| {
            column![
                row![
                    text(tr("ST_DLG_IMPORT_DATE_FORMAT")),
                    text_input(&tr("ST_DLG_CAST_FORMAT_AUTOMATIC"), &self.date_format)
                        .on_input(ImportMessage::DateFormat)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                text(tr("ST_HELP_FORMAT_CODES"))
                    .size(12)
                    .color(tokens.muted_text),
            ]
            .spacing(4)
            .into()
        });
        let body = column![
            text(tr_with("ST_DLG_IMPORT_TITLE", &[("name", &name)])).size(20),
            options,
            text(tr("ST_DLG_IMPORT_PREVIEW")).size(15),
            preview,
        ]
        .extend(date_format)
        .push(
            row![
                text(tr("ST_DLG_IMPORT_ON_ERROR")),
                pick_list(
                    OnError::ALL.map(Labeled),
                    Some(Labeled(self.on_error)),
                    |e| ImportMessage::OnError(e.0)
                )
                .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .push(
            text(tr("ST_DLG_IMPORT_ON_ERROR_NOTE"))
                .size(12)
                .color(tokens.muted_text),
        )
        .push(buttons(
            "ST_DLG_IMPORT_IMPORT",
            ImportMessage::Cancel,
            ImportMessage::Import,
        ))
        .spacing(14);
        card(body.into(), tokens, 820.0)
    }
}
