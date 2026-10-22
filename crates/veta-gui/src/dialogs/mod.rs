//! Dialogs, one per file: column commands (add, rename, choose columns,
//! change type, replace values, format text), filters, sorts, writer
//! settings, metadata, CSV import / export and confirmations. This file
//! holds the widgets they share.

use crate::fonts::{checkbox, pick_list, text, text_input};
use crate::i18n::errors::error_text;
use crate::i18n::model::Labeled;
use crate::i18n::{tr, tr_with};
use iced::widget::{Space, button, container, row, scrollable};
use iced::{Alignment, Border, Element, Length, Theme};
use veta_core::arrow::array::ArrayRef;
use veta_core::arrow::datatypes::SchemaRef;
use veta_core::arrow::datatypes::{DataType, TimeUnit};
use veta_core::convert::{
    CsvExportOptions, CsvOptions, CsvPreview, ExportFormat, TextEncoding, preview_csv,
};
use veta_core::display::type_name;
use veta_core::io::ARROW_SCHEMA_KEY;
use veta_core::model::KeyValue;
use veta_core::steps::{
    Cast, CastOptions, CastPreview, Condition, Filter, FilterOp, OnError, Replace, Sort, SortKey,
    TextOp,
};
use veta_core::steps::{CustomColumn, Merge, Split, SplitBy, Step, count_parts};
use veta_core::values::is_text;
use veta_core::{
    ColumnSettings, Command, Compression, Encoding, FormatVersion, StatisticsLevel, WriterSettings,
};

use crate::settings::theme::Tokens;

mod cast;
mod choose;
mod collect;
mod column;
mod combine;
mod confirm;
mod custom;
mod export;
mod filter;
mod format;
mod import;
mod merge;
mod metadata;
mod replace;
mod rows;
mod sort;
mod split;
mod writer;

pub use cast::*;
pub use choose::*;
pub use collect::*;
pub use column::*;
pub use combine::*;
pub use confirm::*;
pub use custom::*;
pub use export::*;
pub use filter::*;
pub use format::*;
pub use import::*;
pub use merge::*;
pub use metadata::*;
pub use replace::*;
pub use rows::*;
pub use sort::*;
pub use split::*;
pub use writer::*;

/// A dialog that adds a step, or edits an existing one. The app keeps it as
/// `Dialog::Step` and sends it [`StepMessage`]s; whatever the dialog, the
/// step it returns is added (or replaces the edited one) the same way.
pub trait StepDialog: std::fmt::Debug {
    /// Handles `message`; returns the step once the user submits.
    fn update(&mut self, message: StepMessage) -> Option<Step>;
    /// Whether `message` is this dialog's Cancel button.
    fn cancels(&self, message: &StepMessage) -> bool;
    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage>;
    /// Shows an error from adding the step; the dialog stays open. Dialogs
    /// without an error line hand it back, for the error bar.
    fn set_error(&mut self, error: String) -> Option<String> {
        Some(error)
    }
    /// The dialog itself, for tests that look inside it.
    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Messages of the step dialogs, by dialog.
#[derive(Debug, Clone)]
pub enum StepMessage {
    Filter(FilterMessage),
    Sort(SortMessage),
    Cast(CastMessage),
    Replace(ReplaceMessage),
    Format(FormatMessage),
    Split(SplitMessage),
    Merge(MergeMessage),
    Combine(CombineMessage),
    Collect(CollectMessage),
    Custom(CustomMessage),
}

pub(crate) fn card<'a, Message: 'a>(
    content: Element<'a, Message>,
    tokens: Tokens,
    width: f32,
) -> Element<'a, Message> {
    container(content)
        .padding(24)
        .width(width)
        .style(move |_theme: &Theme| {
            container::Style::default()
                .background(tokens.background)
                .color(tokens.text)
                .border(Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: 8.0.into(),
                })
                .shadow(iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25),
                    offset: iced::Vector::new(0.0, 6.0),
                    blur_radius: 24.0,
                })
        })
        .into()
}

/// Cancel and the dialog's action; `action` is the tag of its label.
fn buttons<'a, M: Clone + 'a>(action: &str, cancel: M, submit: M) -> Element<'a, M> {
    row![
        Space::new().width(Length::Fill),
        button(text(tr("ST_COMMON_CANCEL")))
            .style(button::secondary)
            .on_press(cancel),
        button(text(tr(action))).on_press(submit),
    ]
    .spacing(8)
    .into()
}
