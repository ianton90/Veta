//! The iced application: state, messages, update and view. Each area
//! (files, editing, rows, columns, ...) has a file with its message
//! handlers, then its ribbon actions, then its helpers.

mod actions;
mod clipboard;
mod columns;
mod documents;
mod editing;
mod filter;
mod menus;
mod messages;
mod rows;
mod state;
mod stats;
mod steps;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use menus::*;
pub(crate) use messages::*;
pub(crate) use state::*;
pub(crate) use steps::*;
pub(crate) use view::*;

use crate::dialogs::{
    CastDialog, ChooseColumns, ChooseMessage, CollectDialog, ColumnDialog, ColumnMessage,
    CombineDialog, CustomDialog, ExportDialog, ExportMessage, FilterDialog, FormatDialog,
    ImportDialog, ImportMessage, MergeDialog, MetadataDialog, MetadataMessage, ReplaceDialog,
    RowsDialog, RowsMessage, SortDialog, SplitDialog, StepDialog, StepMessage, WriterDialog,
    WriterMessage,
};
use crate::grid::{GRID_ID, GridEvent, GridView, MenuTarget, Nav, Selection};
use crate::platform::files::FileKind;
use crate::platform::{files, icon, instance};
use crate::ribbon::{Action, Ribbon, RibbonMessage};
use crate::settings::config::Config;
#[cfg(test)]
use crate::settings::config::ModePreference;
use crate::settings::theme::{Mode, Tokens, VetaTheme};
use crate::settings::{Draft, Outcome, SettingsMessage};
use crate::settings::{config, theme};
use crate::{dialogs, grid, panes, ribbon, settings};
use iced::widget::{center, container, mouse_area, opaque, pin, row, rule, stack};
use iced::{
    Color, Element, Font, Length, Pixels, Size, Subscription, Task, Theme, event, font, keyboard,
    window,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use veta_core::arrow::array::ArrayRef;
use veta_core::convert::ImportReport;
use veta_core::stats::ColumnStats;
use veta_core::steps::{
    Cast, ComputeJob, Computed, Condition, CustomColumn, Fill, Filter, FilterOp, Merge, OnError,
    Replace, Sort, SortKey, Split, Status, Step, TextOp,
};
use veta_core::{Command, Document, DocumentId, OpenOptions, Workbook, controller};

/// Id of the formula bar's text input.
pub(crate) const FORMULA_ID: iced::widget::Id = iced::widget::Id::new("formula");

/// Id of the text input drawn inside the cell being edited.
pub(crate) const EDITOR_ID: iced::widget::Id = iced::widget::Id::new("cell-editor");

/// Rows dialogs sample (to preview cast failures, suggest split parts).
pub(crate) const SAMPLE_ROWS: usize = 1000;

/// Opens the main window with `files` and blocks until it is closed. If
/// Veta is already running, hands `files` to it and returns at once.
pub fn run(files: Vec<PathBuf>) -> iced::Result {
    if let Some(dir) = instance::default_dir() {
        match instance::claim(&dir, &files) {
            instance::Claim::Forwarded => return Ok(()),
            instance::Claim::First(listener) => instance::listen(listener),
            instance::Claim::Alone => {}
        }
    }
    // Temp files from earlier sessions that crashed.
    std::thread::spawn(veta_core::io::remove_stale_temp_files);
    let startup = Startup::load();
    let (font, size) = startup.font();
    iced::application(
        move || App::new(startup.clone(), files.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .exit_on_close_request(false)
    .settings(iced::Settings {
        default_font: font,
        default_text_size: Pixels(size),
        ..iced::Settings::default()
    })
    .font(icon::FONT_BYTES)
    .window(window_settings())
    .run()
}

/// Names of the columns with nested types.
pub(crate) fn nested_columns(schema: &veta_core::arrow::datatypes::Schema) -> Vec<String> {
    schema
        .fields()
        .iter()
        .filter(|f| f.data_type().is_nested())
        .map(|f| f.name().clone())
        .collect()
}

/// The app icon as 64×64 RGBA, rendered by `packaging/icons/render.sh`.
pub(crate) const APP_ICON: &[u8] = include_bytes!("../../../../packaging/icons/veta-64.rgba");

pub(crate) fn window_settings() -> window::Settings {
    #[allow(unused_mut)]
    let mut settings = window::Settings {
        size: Size::new(1280.0, 800.0),
        icon: window::icon::from_rgba(APP_ICON.to_vec(), 64, 64).ok(),
        ..window::Settings::default()
    };
    // Matches the desktop entry (`veta.desktop`), so the menu entry and the
    // window share the icon.
    #[cfg(target_os = "linux")]
    {
        settings.platform_specific.application_id = "veta".into();
    }
    settings
}

/// Everything read from disk before the window opens.
#[derive(Debug, Clone)]
pub(crate) struct Startup {
    pub(crate) config: Config,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) themes: Vec<VetaTheme>,
    pub(crate) themes_dir: Option<PathBuf>,
    pub(crate) errors: Vec<String>,
}

impl Startup {
    fn load() -> Self {
        let dir = config::config_dir();
        let config_path = dir.as_ref().map(|d| d.join("config.toml"));
        let themes_dir = dir.as_ref().map(|d| d.join("themes"));
        let (config, config_error) = match &config_path {
            Some(path) => Config::load(path),
            None => (Config::default(), None),
        };
        let (themes, mut errors) = theme::load_all(themes_dir.as_deref());
        errors.extend(config_error);
        Self {
            config,
            config_path,
            themes,
            themes_dir,
            errors,
        }
    }

    /// Font and text size for the session. The OS mode is unknown this early,
    /// so "follow system" uses the light theme's font.
    fn font(&self) -> (Font, f32) {
        let theme = pick_theme(&self.themes, &self.config, None);
        let family = self
            .config
            .appearance
            .font_family
            .clone()
            .or_else(|| theme.font_family.clone());
        let size = self.config.appearance.font_size.unwrap_or(theme.font_size);
        let font = match family {
            // iced needs a 'static name; this runs once per process.
            Some(name) => Font::with_name(Box::leak(name.into_boxed_str())),
            None => Font::DEFAULT,
        };
        (font, size)
    }
}

/// The theme for the configured mode, falling back to the built-in one.
pub(crate) fn pick_theme<'a>(
    themes: &'a [VetaTheme],
    config: &Config,
    system: Option<Mode>,
) -> &'a VetaTheme {
    let mode = config.appearance.mode.resolve(system);
    let (wanted, fallback) = match mode {
        Mode::Light => (&config.appearance.light_theme, theme::DEFAULT_LIGHT),
        Mode::Dark => (&config.appearance.dark_theme, theme::DEFAULT_DARK),
    };
    themes
        .iter()
        .find(|t| &t.name == wanted && t.mode == mode)
        .or_else(|| themes.iter().find(|t| t.name == fallback))
        .unwrap_or(&themes[0])
}

pub(crate) fn bold() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }
}

impl App {
    fn new(startup: Startup, files: Vec<PathBuf>) -> (Self, Task<Message>) {
        let (_, text_size) = startup.font();
        let mut app = Self {
            queued_files: Vec::new(),
            window_size: Size::new(1280.0, 800.0),
            workbook: Workbook::new(),
            tabs: Vec::new(),
            active: None,
            show_side_pane: true,
            side_tab: SideTab::default(),
            opening: Vec::new(),
            errors: startup.errors,
            ribbon: Ribbon::default(),
            dialog: None,
            config: startup.config,
            config_path: startup.config_path,
            themes: startup.themes,
            themes_dir: startup.themes_dir,
            system_mode: None,
            text_size,
            edit: None,
            menu: None,
            saving: None,
            closing: None,
            computing: None,
            error_check: None,
            stats: Stats::default(),
            transfers: Vec::new(),
            next_transfer: 0,
            notice: None,
        };
        let open = app.open(files);
        let system = iced::system::theme().map(Message::SystemMode);
        (app, Task::batch([open, system]))
    }

    fn title(&self) -> String {
        match self.active_document() {
            Some((_, doc)) if doc.is_modified() => format!("● {} — Veta", doc.title()),
            Some((_, doc)) => format!("{} — Veta", doc.title()),
            None => "Veta".to_owned(),
        }
    }

    fn current_theme(&self) -> &VetaTheme {
        pick_theme(&self.themes, &self.config, self.system_mode)
    }

    fn theme(&self) -> Theme {
        self.current_theme().iced.clone()
    }

    fn ui(&self) -> Ui {
        Ui {
            tokens: self.current_theme().tokens,
            size: self.text_size,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, status, _window| match event {
            iced::Event::Window(window::Event::FileDropped(path)) => {
                Some(Message::FilesPicked(vec![path]))
            }
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::Escape),
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if modifiers.command() && status == event::Status::Ignored =>
            {
                match key.as_ref() {
                    keyboard::Key::Character("z") if modifiers.shift() => {
                        Some(Message::Action(Action::Redo))
                    }
                    keyboard::Key::Character("z") => Some(Message::Action(Action::Undo)),
                    keyboard::Key::Character("y") => Some(Message::Action(Action::Redo)),
                    keyboard::Key::Character("s") if modifiers.shift() => {
                        Some(Message::Action(Action::SaveAs))
                    }
                    keyboard::Key::Character("s") => Some(Message::Action(Action::Save)),
                    keyboard::Key::Character("c") => Some(Message::Action(Action::Copy)),
                    keyboard::Key::Character("x") => Some(Message::Action(Action::Cut)),
                    keyboard::Key::Character("v") => Some(Message::Action(Action::Paste)),
                    keyboard::Key::Character("n") => Some(Message::Action(Action::New)),
                    keyboard::Key::Character("o") => Some(Message::Action(Action::Open)),
                    keyboard::Key::Character("w") => Some(Message::Action(Action::Close)),
                    keyboard::Key::Character(",") => Some(Message::Action(Action::Settings)),
                    _ => None,
                }
            }
            _ => None,
        });
        Subscription::batch([
            events,
            window::close_requests().map(|_| Message::CloseRequested),
            iced::system::theme_changes().map(Message::SystemMode),
            Subscription::run(instance::incoming).map(Message::Remote),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        // Files sent by another launch wait until a save is done.
        let queued = if self.saving.is_none() && !self.queued_files.is_empty() {
            let files = std::mem::take(&mut self.queued_files);
            self.open(files)
        } else {
            Task::none()
        };
        Task::batch([
            task,
            queued,
            self.schedule_compute(),
            self.schedule_stats(),
            self.schedule_reads(),
        ])
    }

    /// Starts the row reads the grids want, each on its own thread.
    fn schedule_reads(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for tab in &mut self.tabs {
            let Some(read) = tab.grid.take_read() else {
                continue;
            };
            let (document, id) = (tab.id, read.id);
            let (sender, receiver) = iced::futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let _ = sender.send(read.run());
            });
            tasks.push(Task::perform(receiver, move |result| {
                let result = result.unwrap_or_else(|_| Err("the read stopped".into()));
                Message::RowsRead(document, id, result)
            }));
        }
        Task::batch(tasks)
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        if self.saving.is_some()
            && !matches!(
                message,
                Message::Saved(..)
                    | Message::ComputeProgress(..)
                    | Message::ComputeDone(..)
                    | Message::SystemMode(_)
                    | Message::Remote(_)
                    | Message::Grid(_, GridEvent::Resized { .. } | GridEvent::Scroll { .. })
            )
        {
            return Task::none();
        }
        match message {
            m @ Message::Pasted(..) => return self.clipboard_message(m),
            m @ (Message::ColumnDialog(..) | Message::Choose(..)) => {
                return self.columns_message(m);
            }
            m @ (Message::EditInput(..) | Message::CommitEdit) => return self.editing_message(m),
            m @ (Message::OpenDialog
            | Message::FilesPicked(..)
            | Message::Remote(..)
            | Message::Opened(..)
            | Message::Imported(..)
            | Message::SaveTo(..)
            | Message::Saved(..)
            | Message::SelectTab(..)
            | Message::CloseTab(..)
            | Message::CloseRequested
            | Message::CloseAnswer(..)
            | Message::ImportPicked(..)
            | Message::Import(..)
            | Message::Export(..)
            | Message::ExportTo(..)
            | Message::Exported(..)
            | Message::TransferProgress(..)
            | Message::TransferDone(..)
            | Message::CancelTransfer(..)
            | Message::SaveRecipe
            | Message::SaveRecipeTo(..)
            | Message::LoadRecipe
            | Message::LoadRecipeFrom(..)
            | Message::RecipeChoice(..)
            | Message::Writer(..)
            | Message::Metadata(..)) => return self.documents_message(m),
            m @ Message::ClearFilter => {
                return self.filter_message(m);
            }
            m @ (Message::Menu(..) | Message::CloseMenu) => return self.menus_message(m),
            m @ Message::Rows(..) => return self.rows_message(m),
            m @ (Message::StatsProgress(..) | Message::StatsDone(..)) => {
                return self.stats_message(m);
            }
            m @ (Message::Step(..)
            | Message::ComputeProgress(..)
            | Message::ComputeDone(..)
            | Message::CancelCompute
            | Message::PreviewStep(..)
            | Message::RemoveStep(..)
            | Message::MoveStep(..)
            | Message::EditStep(..)
            | Message::ConfirmStep(..)) => return self.steps_message(m),
            Message::Action(action) => {
                self.notice = None;
                return self.perform(action);
            }
            Message::Ribbon(message) => self.ribbon.update(message),
            Message::RowsRead(id, read, result) => {
                if let (Some(doc), Some(tab)) = (
                    self.workbook.get(id),
                    self.tabs.iter_mut().find(|t| t.id == id),
                ) {
                    tab.grid.install_read(read, result, doc);
                }
            }
            Message::WindowResized(size) => self.window_size = size,
            Message::Grid(id, event)
                if !self.editable(id)
                    && matches!(
                        event,
                        GridEvent::StartEdit { .. }
                            | GridEvent::ClearCell
                            | GridEvent::InsertRows
                            | GridEvent::DeleteRows
                            | GridEvent::MoveColumn { .. }
                    ) =>
            {
                if !matches!(event, GridEvent::StartEdit { .. }) {
                    self.not_editable(id);
                }
            }
            Message::Grid(id, event) => match event {
                GridEvent::StartEdit { initial } => return self.start_edit(id, initial),
                GridEvent::EditMove(nav) => {
                    if self.commit_edit() {
                        self.grid_event(id, GridEvent::Navigate(nav));
                        return iced::widget::operation::focus(GRID_ID);
                    }
                }
                GridEvent::InsertRows | GridEvent::DeleteRows
                    if let Some(item) = self.selected_cell_item(id) =>
                {
                    let change = if event == GridEvent::InsertRows {
                        ItemChange::InsertAbove
                    } else {
                        ItemChange::Remove
                    };
                    return self.insert_or_remove_item(id, &item, change);
                }
                GridEvent::InsertRows => self.insert_rows(id, false),
                GridEvent::MoveColumn { from, to } => self.move_column(id, from, to),
                GridEvent::DeleteRows => self.delete_rows(id),
                GridEvent::ContextMenu { target, position } => {
                    self.commit_edit();
                    self.menu = Some(ContextMenu {
                        document: id,
                        target,
                        position,
                    });
                }
                GridEvent::ClearCell if let Some(item) = self.selected_cell_item(id) => {
                    if !item.item.is_slot() {
                        match self.set_item_value(id, &item, None) {
                            Ok(()) => self.refresh(id),
                            Err(e) => self.errors.push(e),
                        }
                    }
                }
                GridEvent::ClearCell => {
                    if let Some((row, column)) = self.selected_cell(id) {
                        match self.selected_struct_field(id) {
                            Some(field) => match self.set_field(id, row, &column, &field, None) {
                                Ok(()) => self.refresh(id),
                                Err(e) => self.errors.push(e),
                            },
                            None => {
                                self.execute(
                                    id,
                                    Command::SetCell {
                                        row,
                                        column,
                                        value: None,
                                    },
                                );
                            }
                        }
                    }
                }
                event => {
                    if matches!(
                        event,
                        GridEvent::Select { .. }
                            | GridEvent::SelectItem { .. }
                            | GridEvent::SelectRows { .. }
                            | GridEvent::Navigate(_)
                    ) {
                        self.commit_edit_or_drop();
                    }
                    if let (Some(doc), Some(tab)) = (
                        self.workbook.get(id),
                        self.tabs.iter_mut().find(|t| t.id == id),
                    ) {
                        tab.grid.apply(event, doc);
                    }
                }
            },
            Message::SideTab(side) => self.side_tab = side,
            Message::Escape => {
                // Close the innermost thing that is open.
                if self.menu.take().is_some() {
                } else if self.dialog.is_some() {
                    self.dialog = None;
                    self.closing = None;
                } else if self.edit.take().is_some() {
                    return iced::widget::operation::focus(GRID_ID);
                }
            }
            Message::DismissError(index) => {
                if index < self.errors.len() {
                    self.errors.remove(index);
                }
            }
            Message::Settings(message) => {
                if let Some(Dialog::Settings(draft)) = &mut self.dialog {
                    match draft.update(message, &self.config) {
                        Outcome::Editing => {}
                        Outcome::Cancelled => self.dialog = None,
                        Outcome::Saved(config) => {
                            self.config = *config;
                            self.save_config();
                            self.dialog = None;
                        }
                    }
                }
            }
            Message::CloseDialog => {
                self.dialog = None;
                self.closing = None;
            }
            Message::SystemMode(mode) => {
                self.system_mode = match mode {
                    iced::theme::Mode::Light => Some(Mode::Light),
                    iced::theme::Mode::Dark => Some(Mode::Dark),
                    iced::theme::Mode::None => None,
                };
            }
        }
        Task::none()
    }

    /// Runs a command on a document and refreshes its grid. Errors are
    /// shown to the user. Returns whether the command succeeded.
    fn execute(&mut self, id: DocumentId, command: Command) -> bool {
        let Some(doc) = self.workbook.get_mut(id) else {
            return false;
        };
        match controller::execute(doc, command) {
            Ok(()) => {
                self.refresh(id);
                true
            }
            Err(e) => {
                self.errors.push(e.to_string());
                false
            }
        }
    }

    /// Re-reads a document's grid after a change, showing all steps.
    fn refresh(&mut self, id: DocumentId) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.set_preview(None, doc);
        }
    }

    /// Whether the document can be changed: all steps shown and evaluated.
    fn editable(&self, id: DocumentId) -> bool {
        let previewing = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .is_some_and(|t| t.grid.preview().is_some() || t.grid.filter().is_some());
        let ready = self
            .workbook
            .get(id)
            .is_some_and(|d| d.status() == &Status::Ready);
        !previewing && ready
    }

    /// Whether steps can be removed, moved or edited: not while one is
    /// computing or a filter is on (broken steps can be fixed).
    fn steps_changeable(&self, id: DocumentId) -> bool {
        let computing = self
            .workbook
            .get(id)
            .is_some_and(|d| matches!(d.status(), Status::Computing { .. }));
        let filtered = self
            .tabs
            .iter()
            .any(|t| t.id == id && t.grid.filter().is_some());
        !computing && !filtered
    }

    /// Explains why a change can't be made right now.
    fn not_editable(&mut self, id: DocumentId) {
        let message = match self.workbook.get(id).map(Document::status) {
            Some(Status::Broken { step, .. }) => {
                format!(
                    "Step {} has an error. Fix or remove it to make changes.",
                    step + 1
                )
            }
            Some(Status::Computing { step }) => {
                format!(
                    "Step {} is still being computed. Wait for it, or cancel it.",
                    step + 1
                )
            }
            _ if self
                .tabs
                .iter()
                .any(|t| t.id == id && t.grid.filter().is_some()) =>
            {
                "A filter is on. Keep or remove the filtered rows, or clear the filter, to make \
                 changes."
                    .into()
            }
            _ => "You're viewing an earlier step. Select the last step to make changes.".into(),
        };
        if !self.errors.contains(&message) {
            self.errors.push(message);
        }
    }

    fn grid_event(&mut self, id: DocumentId, event: GridEvent) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.apply(event, doc);
        }
    }

    fn save_config(&mut self) {
        if let Some(path) = &self.config_path
            && let Err(e) = self.config.save(path)
        {
            self.errors.push(format!("Could not save settings: {e}"));
        }
    }

    fn active_document(&self) -> Option<(&Tab, &Document)> {
        let id = self.active?;
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        Some((tab, self.workbook.get(id)?))
    }
}
