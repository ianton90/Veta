//! Veta's desktop interface, built with iced.
//!
//! The iced state holds the core [`Workbook`] (the model) plus GUI-only state.
//! Messages that change data are turned into core commands; see
//! `docs/ARCHITECTURE.md`.

use iced::widget::{center, column, text};
use iced::{Element, Size};
use veta_core::Workbook;

/// Opens the main window and blocks until it is closed.
pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("Veta")
        .window_size(Size::new(1200.0, 800.0))
        .run()
}

#[derive(Debug, Default)]
struct App {
    workbook: Workbook,
}

#[derive(Debug, Clone)]
enum Message {}

impl App {
    fn new() -> Self {
        Self::default()
    }

    fn update(&mut self, message: Message) {
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        if self.workbook.is_empty() {
            center(
                column![
                    text("No file open").size(24),
                    text("Open a Parquet file to start.")
                ]
                .spacing(8),
            )
            .into()
        } else {
            center(text(format!("{} file(s) open", self.workbook.len()))).into()
        }
    }
}
