//! Dialogs, one per file: column commands (add, rename, choose columns,
//! change type, replace values, format text), filters, sorts, writer
//! settings, metadata, CSV import / export and confirmations. This file
//! holds the widgets they share.

use iced::widget::{
    Space, button, checkbox, container, pick_list, row, scrollable, text, text_input,
};
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
    Cast, CastOptions, CastPreview, Condition, FORMAT_CODES, Filter, FilterOp, OnError, Replace,
    Sort, SortKey, TextOp,
};
use veta_core::steps::{CustomColumn, Merge, Split, SplitBy, count_parts, expression_help};
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
mod json;
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
pub use json::*;
pub use merge::*;
pub use metadata::*;
pub use replace::*;
pub use rows::*;
pub use sort::*;
pub use split::*;
pub use writer::*;

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
