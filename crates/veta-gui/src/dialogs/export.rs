//! Export to CSV or JSON.

use super::*;
use iced::widget::column;

/// Formats offered for exporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportChoice {
    Csv,
    JsonLines,
    JsonArray,
}

impl std::fmt::Display for ExportChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Csv => "CSV",
            Self::JsonLines => "JSON, one object per line",
            Self::JsonArray => "JSON array",
        })
    }
}

/// Export the document (after all steps) to CSV or JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDialog {
    pub choice: ExportChoice,
    delimiter: Delimiter,
    other: String,
    quote: Quote,
    error: Option<String>,
    decimal: DecimalMark,
    header: bool,
}

#[derive(Debug, Clone)]
pub enum ExportMessage {
    Format(ExportChoice),
    Delimiter(Delimiter),
    Other(String),
    Quote(Quote),
    Decimal(DecimalMark),
    Header(bool),
    Export,
    Cancel,
}

impl Default for ExportDialog {
    fn default() -> Self {
        Self {
            choice: ExportChoice::Csv,
            delimiter: Delimiter::Comma,
            other: String::new(),
            quote: Quote::Double,
            error: None,
            decimal: DecimalMark::Dot,
            header: true,
        }
    }
}

impl ExportDialog {
    pub fn format(&self) -> ExportFormat {
        match self.choice {
            ExportChoice::Csv => ExportFormat::Csv(CsvExportOptions {
                delimiter: separator(self.delimiter, &self.other).unwrap_or(b','),
                quote: self.quote.byte(),
                header: self.header,
                decimal: self.decimal.char(),
            }),
            ExportChoice::JsonLines => ExportFormat::JsonLines,
            ExportChoice::JsonArray => ExportFormat::JsonArray,
        }
    }

    /// Handles a message. Returns the format on submit.
    pub fn update(&mut self, message: ExportMessage) -> Option<ExportFormat> {
        self.error = None;
        match message {
            ExportMessage::Format(c) => self.choice = c,
            ExportMessage::Other(text) => self.other = text,
            ExportMessage::Quote(q) => self.quote = q,
            ExportMessage::Delimiter(d) => {
                self.delimiter = d;
                if d == Delimiter::Semicolon {
                    self.decimal = DecimalMark::Comma;
                }
            }
            ExportMessage::Decimal(d) => self.decimal = d,
            ExportMessage::Header(on) => self.header = on,
            ExportMessage::Cancel => {}
            ExportMessage::Export => {
                if self.choice == ExportChoice::Csv
                    && let Err(e) = separator(self.delimiter, &self.other)
                {
                    self.error = Some(e);
                    return None;
                }
                return Some(self.format());
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ExportMessage> {
        let labelled = |label: &'static str, control: Element<'static, ExportMessage>| {
            row![text(label).width(110), control]
                .spacing(12)
                .align_y(Alignment::Center)
        };
        let mut body = column![
            text("Export").size(20),
            labelled(
                "Format",
                pick_list(
                    [
                        ExportChoice::Csv,
                        ExportChoice::JsonLines,
                        ExportChoice::JsonArray
                    ],
                    Some(self.choice),
                    ExportMessage::Format
                )
                .width(Length::Fill)
                .into()
            ),
        ]
        .spacing(14);
        if self.choice == ExportChoice::Csv {
            body = body
                .push(labelled(
                    "Separator",
                    row![
                        pick_list(
                            Delimiter::ALL,
                            Some(self.delimiter),
                            ExportMessage::Delimiter,
                        )
                        .width(Length::Fill)
                    ]
                    .extend((self.delimiter == Delimiter::Other).then(|| {
                        text_input("", &self.other)
                            .on_input(ExportMessage::Other)
                            .width(36)
                            .into()
                    }))
                    .spacing(4)
                    .into(),
                ))
                .push(labelled(
                    "Quote",
                    pick_list(Quote::ALL, Some(self.quote), ExportMessage::Quote)
                        .width(Length::Fill)
                        .into(),
                ))
                .push(labelled(
                    "Decimal",
                    pick_list(
                        [DecimalMark::Dot, DecimalMark::Comma],
                        Some(self.decimal),
                        ExportMessage::Decimal,
                    )
                    .width(Length::Fill)
                    .into(),
                ))
                .push(
                    checkbox(self.header)
                        .label("Write column names in the first row")
                        .on_toggle(ExportMessage::Header),
                );
        }
        body = body
            .push(text("Exports the data after all applied steps.").color(tokens.muted_text))
            .push(buttons(
                "Export…",
                ExportMessage::Cancel,
                ExportMessage::Export,
            ));
        card(body.into(), tokens, 460.0)
    }
}
