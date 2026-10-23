//! The iced application: state, messages, update and view. Each area
//! (files, editing, rows, columns, ...) has a file with its message
//! handlers, then its ribbon actions, then its helpers.

mod actions;
mod clipboard;
mod columns;
mod documents;
mod editing;
mod file_settings;
mod filter;
mod menus;
mod messages;
mod recipes;
mod rows;
mod state;
mod stats;
mod steps;
#[cfg(test)]
mod tests;
mod transfers;
mod view;

pub(crate) use menus::*;
pub(crate) use messages::*;
pub(crate) use state::*;
pub(crate) use steps::*;
pub(crate) use view::*;

use crate::dialogs::{
    CastDialog, ChooseColumns, ChooseMessage, CollectDialog, ColumnDialog, CombineDialog,
    CustomDialog, ExportDialog, ExportMessage, FilterDialog, FormatDialog, ImportDialog,
    ImportMessage, MergeDialog, MetadataDialog, MetadataMessage, ReplaceDialog, RowsDialog,
    RowsMessage, SortDialog, SplitDialog, StepDialog, StepMessage, WriterDialog, WriterMessage,
};
use crate::grid::{GRID_ID, GridEvent, GridView, MenuTarget, Nav, Selection};
use crate::i18n::errors::{error_text, io_text};
use crate::i18n::{tr, tr_with};
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
    Color, Element, Font, Length, Pixels, Size, Subscription, Task, Theme, event, keyboard, window,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use veta_core::arrow::array::ArrayRef;
use veta_core::convert::ImportReport;
use veta_core::stats::ColumnStats;
use veta_core::steps::{
    Cast, ComputeJob, Computed, Condition, CustomColumn, Fill, Filter, FilterOp, Merge, Replace,
    Sort, SortKey, Split, Status, Step, TextOp,
};
use veta_core::{Command, Document, DocumentId, OpenOptions, Workbook, controller};

/// Id of the formula bar's text input.
pub(crate) const FORMULA_ID: iced::widget::Id = iced::widget::Id::new("formula");

/// Id of the text input drawn inside the cell being edited.
pub(crate) const EDITOR_ID: iced::widget::Id = iced::widget::Id::new("cell-editor");

/// How long the cell editor flashes after refused input.
const FLASH: std::time::Duration = std::time::Duration::from_millis(500);

/// Rows a dialog samples to preview its result (cast failures, split
/// parts).
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
        // Messages from here on are in the chosen language.
        crate::i18n::Translations::set_language(config.language());
        let (themes, mut errors) = theme::load_all(themes_dir.as_deref());
        errors.extend(config_error);
        errors.extend(missing_font(&config, pick_theme(&themes, &config, None)));
        Self {
            config,
            config_path,
            themes,
            themes_dir,
            errors,
        }
    }

    /// iced's default font and text size, for the first frame (the app sets
    /// its own on every frame, see `crate::fonts`). The OS mode is unknown
    /// this early, so "follow system" uses the light theme's font.
    fn font(&self) -> (Font, f32) {
        interface_font(&self.config, pick_theme(&self.themes, &self.config, None))
    }
}

/// The interface font and text size: the settings', else the theme's.
fn interface_font(config: &Config, theme: &VetaTheme) -> (Font, f32) {
    (
        crate::fonts::resolve(font_family(config, theme)),
        config.appearance.font_size.unwrap_or(theme.font_size),
    )
}

/// The font family the settings name, else the theme's.
fn font_family<'a>(config: &'a Config, theme: &'a VetaTheme) -> Option<&'a str> {
    config
        .appearance
        .font_family
        .as_deref()
        .or(theme.font_family.as_deref())
}

/// A message when the font the settings or theme name isn't installed.
pub(crate) fn missing_font(config: &Config, theme: &VetaTheme) -> Option<String> {
    let family = font_family(config, theme)?;
    crate::fonts::find(family)
        .is_none()
        .then(|| tr_with("ST_MSG_FONT_MISSING", &[("font", &family)]))
}

/// A document's name as shown on its tab and in the window title, marked
/// with ● when it has unsaved changes.
pub(crate) fn doc_label(doc: &Document) -> String {
    let title = doc.title_or(&tr("ST_DOC_UNTITLED"));
    match doc.is_modified() {
        true => format!("● {title}"),
        false => title,
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

impl App {
    fn new(startup: Startup, files: Vec<PathBuf>) -> (Self, Task<Message>) {
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
            edit: None,
            flash: None,
            menu: None,
            saving: None,
            closing: None,
            computing: None,
            stats: Stats::default(),
            transfers: Vec::new(),
            next_transfer: 0,
            notice: None,
        };
        app.config.apply_memory_budget();
        crate::i18n::Translations::set_language(app.config.language());
        let open = app.open(files);
        let system = iced::system::theme().map(Message::SystemMode);
        (app, Task::batch([open, system]))
    }

    fn title(&self) -> String {
        match self.active_document() {
            Some((_, doc)) => format!("{} — Veta", doc_label(doc)),
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
            size: self.interface_font().1,
        }
    }

    /// The interface font and text size: the settings', else the theme's.
    pub(crate) fn interface_font(&self) -> (Font, f32) {
        interface_font(&self.config, self.current_theme())
    }

    /// The table's text size: its own setting (else the interface's),
    /// zoomed.
    pub(crate) fn table_text_size(&self) -> f32 {
        let a = &self.config.appearance;
        let base = a.table_font_size.unwrap_or(self.interface_font().1);
        base * f32::from(a.zoom) / 100.0
    }

    /// Gives every table the current text size; run after every update.
    fn apply_table_size(&mut self) {
        let size = self.table_text_size();
        for tab in &mut self.tabs {
            if let Some(doc) = self.workbook.get(tab.id) {
                tab.grid.set_text_size(size, doc);
            }
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
                    keyboard::Key::Character(c) => {
                        Action::for_key(c, modifiers.shift()).map(Message::Action)
                    }
                    _ => None,
                }
            }
            _ => None,
        });
        // Redrawn every frame while the cell editor flashes.
        let flash = match self.flash {
            Some(_) => window::frames().map(Message::Frame),
            None => Subscription::none(),
        };
        Subscription::batch([
            events,
            flash,
            window::close_requests().map(|_| Message::CloseRequested),
            iced::system::theme_changes().map(Message::SystemMode),
            Subscription::run(instance::incoming).map(Message::Remote),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        // Tables follow the zoom, fonts and theme, whatever changed them
        // (nothing happens when the size is the same).
        self.apply_table_size();
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
                let result = result.unwrap_or(Err(veta_core::Error::Cancelled));
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
        match message.area() {
            Area::App => {}
            Area::Clipboard => return self.clipboard_message(message),
            Area::Columns => return self.columns_message(message),
            Area::Documents => return self.documents_message(message),
            Area::Editing => return self.editing_message(message),
            Area::FileSettings => return self.file_settings_message(message),
            Area::Filter => return self.filter_message(message),
            Area::Menus => return self.menus_message(message),
            Area::Recipes => return self.recipes_message(message),
            Area::Rows => return self.rows_message(message),
            Area::Stats => return self.stats_message(message),
            Area::Steps => return self.steps_message(message),
            Area::Transfers => return self.transfers_message(message),
        }
        match message {
            Message::Action(action) => {
                self.notice = None;
                return self.perform(action);
            }
            Message::Ribbon(message) => self.ribbon.update(message),
            Message::RowsRead(id, read, result) => {
                self.grid_do(id, |grid, doc| grid.install_read(read, result, doc));
            }
            Message::WindowResized(size) => self.window_size = size,
            Message::Frame(now) => {
                self.flash = self.flash.and_then(|(start, _)| {
                    let elapsed = now.saturating_duration_since(start);
                    (elapsed < FLASH)
                        .then(|| (start, 1.0 - elapsed.as_secs_f32() / FLASH.as_secs_f32()))
                });
            }
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
                GridEvent::Zoom(steps) => self.zoom_by(steps),
                GridEvent::StartEdit { initial } => return self.start_edit(id, initial),
                GridEvent::EditMove(nav) => {
                    if self.commit_edit() {
                        self.grid_do(id, |grid, doc| grid.apply(GridEvent::Navigate(nav), doc));
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
                GridEvent::ClearCell => {
                    let on_slot = self
                        .selected_cell_item(id)
                        .is_some_and(|item| item.item.is_slot());
                    if let Some(value) = self.selected_value(id).filter(|_| !on_slot) {
                        match self.set_value(id, &value, None) {
                            Ok(()) => self.refresh(id),
                            Err(e) => self.errors.push(e),
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
                    self.grid_do(id, |grid, doc| grid.apply(event, doc));
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
                            let language = config.language();
                            let changed = language != self.config.language();
                            let missing = missing_font(&self.config, self.current_theme());
                            self.config = *config;
                            if missing.is_some()
                                && missing_font(&self.config, self.current_theme()).is_none()
                            {
                                self.errors.retain(|e| Some(e) != missing.as_ref());
                            }
                            self.config.apply_memory_budget();
                            self.save_config();
                            self.dialog = None;
                            if changed {
                                return Task::done(Message::LanguageChanged(language));
                            }
                        }
                    }
                }
            }
            Message::LanguageChanged(language) => {
                crate::i18n::Translations::set_language(language);
                // The grid keeps the text of the rows it read ("[2 items]").
                for tab in &mut self.tabs {
                    if let Some(doc) = self.workbook.get(tab.id) {
                        tab.grid.refresh(doc);
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
            _ => unreachable!("handled by its area"),
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
                self.errors.push(error_text(&e));
                false
            }
        }
    }

    /// Re-reads a document's grid after a change, showing all steps.
    fn refresh(&mut self, id: DocumentId) {
        self.grid_do(id, |grid, doc| grid.set_preview(None, doc));
    }

    /// Whether the document can be changed: all steps shown and evaluated.
    fn editable(&self, id: DocumentId) -> bool {
        let previewing = self
            .tab(id)
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
        let filtered = self.tab(id).is_some_and(|t| t.grid.filter().is_some());
        !computing && !filtered
    }

    /// Explains why a change can't be made right now.
    fn not_editable(&mut self, id: DocumentId) {
        let message = match self.workbook.get(id).map(Document::status) {
            Some(Status::Broken { step, .. }) => {
                tr_with("ST_MSG_STEP_BROKEN", &[("n", &(step + 1))])
            }
            Some(Status::Computing { step }) => {
                tr_with("ST_MSG_STEP_COMPUTING", &[("n", &(step + 1))])
            }
            _ if self.tab(id).is_some_and(|t| t.grid.filter().is_some()) => tr("ST_MSG_FILTER_ON"),
            _ => tr("ST_MSG_EARLIER_STEP"),
        };
        if !self.errors.contains(&message) {
            self.errors.push(message);
        }
    }

    fn save_config(&mut self) {
        if let Some(path) = &self.config_path
            && let Err(e) = self.config.save(path)
        {
            self.errors
                .push(tr_with("ST_MSG_SETTINGS_SAVE_FAILED", &[("error", &e)]));
        }
    }

    fn active_document(&self) -> Option<(&Tab, &Document)> {
        let id = self.active?;
        let tab = self.tab(id)?;
        Some((tab, self.workbook.get(id)?))
    }
}
