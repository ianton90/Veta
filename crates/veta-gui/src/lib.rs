//! Veta's desktop interface, built with iced.
//!
//! The iced state holds the core [`Workbook`] (the model) plus GUI-only state
//! (tabs, grid scroll positions, panes, settings). Messages that change data
//! will be turned into core commands; see `docs/ARCHITECTURE.md`.

mod config;
mod dialogs;
mod grid;
mod icon;
mod panes;
mod ribbon;
mod settings;
mod theme;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iced::widget::{center, column, container, mouse_area, opaque, pin, row, rule, stack};
use iced::{
    Color, Element, Font, Length, Pixels, Size, Subscription, Task, Theme, event, font, keyboard,
    window,
};
use veta_core::{Command, Document, DocumentId, OpenOptions, Workbook, controller};

use crate::config::Config;
#[cfg(test)]
use crate::config::ModePreference;
use crate::dialogs::{
    ChooseColumns, ChooseMessage, ColumnDialog, ColumnMessage, MetadataDialog, MetadataMessage,
    WriterDialog, WriterMessage,
};
use crate::grid::{GRID_ID, GridEvent, GridView, MenuTarget, Nav};
use crate::ribbon::{Action, Ribbon, RibbonMessage};
use crate::settings::{Draft, Outcome, SettingsMessage};
use crate::theme::{Mode, Tokens, VetaTheme};

/// Id of the formula bar's text input.
const FORMULA_ID: iced::widget::Id = iced::widget::Id::new("formula");

/// Opens the main window with `files` and blocks until it is closed.
pub fn run(files: Vec<PathBuf>) -> iced::Result {
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
    .window_size(Size::new(1280.0, 800.0))
    .run()
}

/// Everything read from disk before the window opens.
#[derive(Debug, Clone)]
struct Startup {
    config: Config,
    config_path: Option<PathBuf>,
    themes: Vec<VetaTheme>,
    themes_dir: Option<PathBuf>,
    errors: Vec<String>,
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
fn pick_theme<'a>(themes: &'a [VetaTheme], config: &Config, system: Option<Mode>) -> &'a VetaTheme {
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

fn bold() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }
}

/// Theme colors and base text size, passed to view functions.
#[derive(Debug, Clone, Copy)]
struct Ui {
    tokens: Tokens,
    size: f32,
}

impl Ui {
    fn small(self) -> f32 {
        (self.size - 1.0).max(8.0)
    }

    fn heading(self) -> f32 {
        self.size + 2.0
    }
}

#[derive(Debug)]
struct App {
    workbook: Workbook,
    /// GUI state per open document, in tab order.
    tabs: Vec<Tab>,
    active: Option<DocumentId>,
    show_side_pane: bool,
    /// Files currently being opened in the background.
    opening: Vec<PathBuf>,
    errors: Vec<String>,
    ribbon: Ribbon,
    dialog: Option<Dialog>,
    config: Config,
    config_path: Option<PathBuf>,
    themes: Vec<VetaTheme>,
    themes_dir: Option<PathBuf>,
    /// OS light/dark mode, once known.
    system_mode: Option<Mode>,
    text_size: f32,
    /// Cell being edited in the formula bar.
    edit: Option<Edit>,
    menu: Option<ContextMenu>,
    /// Document being saved; the window is blocked meanwhile.
    saving: Option<DocumentId>,
    /// Tabs being closed, waiting for "save changes?" answers.
    closing: Option<Closing>,
}

#[derive(Debug, Clone, PartialEq)]
struct Closing {
    /// Documents still to close, in order. The first one is being asked
    /// about when it has unsaved changes.
    queue: Vec<DocumentId>,
    /// Quit the app once all are closed.
    exit: bool,
}

/// Answer to "save changes before closing?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseAnswer {
    Save,
    Discard,
    Cancel,
}

/// An open right-click menu.
#[derive(Debug, Clone, PartialEq)]
struct ContextMenu {
    document: DocumentId,
    target: MenuTarget,
    position: iced::Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    EditCell,
    ClearCell,
    InsertRowsAbove,
    InsertRowsBelow,
    DeleteRows,
    InsertColumnLeft,
    InsertColumnRight,
    RenameColumn,
    RemoveColumns,
    MoveColumnLeft,
    MoveColumnRight,
}

#[derive(Debug, Clone, PartialEq)]
struct Edit {
    document: DocumentId,
    row: usize,
    column: String,
    text: String,
}

#[derive(Debug)]
struct Tab {
    id: DocumentId,
    grid: GridView,
}

#[derive(Debug)]
enum Dialog {
    Settings(Draft),
    About,
    Column {
        document: DocumentId,
        dialog: ColumnDialog,
    },
    Choose {
        document: DocumentId,
        dialog: ChooseColumns,
    },
    Writer {
        document: DocumentId,
        dialog: WriterDialog,
    },
    Metadata {
        document: DocumentId,
        dialog: MetadataDialog,
    },
    ConfirmClose {
        document: DocumentId,
    },
}

#[derive(Debug, Clone)]
enum Message {
    Action(Action),
    Ribbon(RibbonMessage),
    FilesPicked(Vec<PathBuf>),
    Opened(PathBuf, Result<Loaded, String>),
    /// Save As dialog closed (with the chosen path, if any).
    SaveTo(DocumentId, Option<PathBuf>),
    Saved(DocumentId, Result<Slot<veta_core::SaveResult>, String>),
    SelectTab(DocumentId),
    CloseTab(DocumentId),
    Grid(DocumentId, GridEvent),
    DismissError(usize),
    Settings(SettingsMessage),
    CloseDialog,
    CloseRequested,
    CloseAnswer(CloseAnswer),
    /// Esc: closes a dialog or cancels an edit.
    Escape,
    EditInput(String),
    CommitEdit,
    Menu(MenuItem),
    CloseMenu,
    ColumnDialog(ColumnMessage),
    Choose(ChooseMessage),
    Writer(WriterMessage),
    Metadata(MetadataMessage),
    SystemMode(iced::theme::Mode),
    OpenDialog,
}

/// A value produced on a worker thread. Messages must be `Clone`; documents
/// and save results are not, so they travel in a take-once slot.
#[derive(Debug)]
struct Slot<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Slot<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

type Loaded = Slot<Document>;

impl App {
    fn new(startup: Startup, files: Vec<PathBuf>) -> (Self, Task<Message>) {
        let (_, text_size) = startup.font();
        let mut app = Self {
            workbook: Workbook::new(),
            tabs: Vec::new(),
            active: None,
            show_side_pane: true,
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
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        if self.saving.is_some()
            && !matches!(
                message,
                Message::Saved(..)
                    | Message::SystemMode(_)
                    | Message::Grid(_, GridEvent::Resized { .. } | GridEvent::Scroll { .. })
            )
        {
            return Task::none();
        }
        match message {
            Message::Action(action) => return self.perform(action),
            Message::OpenDialog => return self.perform(Action::Open),
            Message::Ribbon(message) => self.ribbon.update(message),
            Message::FilesPicked(files) => return self.open(files),
            Message::Opened(path, result) => {
                self.opening.retain(|p| p != &path);
                match result.map(|loaded| loaded.take()) {
                    Ok(Some(document)) => {
                        let grid = GridView::new(&document, self.text_size);
                        let id = self.workbook.add(document);
                        self.tabs.push(Tab { id, grid });
                        self.active = Some(id);
                        self.config.add_recent(&path);
                        self.save_config();
                    }
                    Ok(None) => {}
                    Err(e) => self
                        .errors
                        .push(format!("Could not open {}: {e}", path.display())),
                }
            }
            Message::SaveTo(id, Some(path)) => return self.start_save(id, Some(path)),
            Message::SaveTo(_, None) => {}
            Message::Saved(id, result) => {
                self.saving = None;
                match result.map(|slot| slot.take()) {
                    Ok(Some(saved)) => {
                        if let Some(doc) = self.workbook.get_mut(id) {
                            doc.finish_save(saved);
                            if let Some(path) = doc.path().map(Path::to_path_buf) {
                                self.config.add_recent(&path);
                                self.save_config();
                            }
                        }
                        self.refresh(id);
                        if self
                            .closing
                            .as_ref()
                            .is_some_and(|c| c.queue.first() == Some(&id))
                        {
                            return self.continue_closing();
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.errors.push(format!("Could not save: {e}"));
                        self.closing = None;
                    }
                }
            }
            Message::SelectTab(id) => self.active = Some(id),
            Message::CloseTab(id) => return self.request_close(vec![id], false),
            Message::CloseRequested => {
                let all = self.tabs.iter().map(|t| t.id).collect();
                return self.request_close(all, true);
            }
            Message::CloseAnswer(answer) => return self.answer_close(answer),
            Message::Grid(id, event) => match event {
                GridEvent::StartEdit { initial } => return self.start_edit(id, initial),
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
                    if let Some((row, column)) = self.selected_cell(id) {
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
                event => {
                    if matches!(
                        event,
                        GridEvent::Select { .. }
                            | GridEvent::SelectRows { .. }
                            | GridEvent::Navigate(_)
                    ) {
                        self.commit_edit();
                    }
                    if let (Some(doc), Some(tab)) = (
                        self.workbook.get(id),
                        self.tabs.iter_mut().find(|t| t.id == id),
                    ) {
                        tab.grid.apply(event, doc);
                    }
                }
            },
            Message::EditInput(text) => match &mut self.edit {
                Some(edit) => edit.text = text,
                None => {
                    // Typing straight into the formula bar starts an edit.
                    if let Some(id) = self.active {
                        let task = self.start_edit(id, Some(text));
                        return task;
                    }
                }
            },
            Message::CommitEdit => {
                if self.commit_edit()
                    && let Some(id) = self.active
                {
                    self.grid_event(id, GridEvent::Navigate(Nav::Down));
                    return iced::widget::operation::focus(GRID_ID);
                }
            }
            Message::Menu(item) => {
                let Some(menu) = self.menu.take() else {
                    return Task::none();
                };
                let id = menu.document;
                match item {
                    MenuItem::EditCell => return self.start_edit(id, None),
                    MenuItem::ClearCell => {
                        return self.update(Message::Grid(id, GridEvent::ClearCell));
                    }
                    MenuItem::InsertRowsAbove => self.insert_rows(id, false),
                    MenuItem::InsertRowsBelow => self.insert_rows(id, true),
                    MenuItem::DeleteRows => self.delete_rows(id),
                    MenuItem::InsertColumnLeft | MenuItem::InsertColumnRight => {
                        let columns = self.selected_columns(id);
                        let at = match (item, columns) {
                            (MenuItem::InsertColumnLeft, Some(c)) => c.start,
                            (_, Some(c)) => c.end,
                            (_, None) => 0,
                        };
                        return self.open_column_dialog(id, ColumnDialog::add(at));
                    }
                    MenuItem::RenameColumn => return self.rename_column(id),
                    MenuItem::RemoveColumns => self.remove_columns(id),
                    MenuItem::MoveColumnLeft | MenuItem::MoveColumnRight => {
                        if let Some(c) = self.selected_columns(id) {
                            let to = if item == MenuItem::MoveColumnLeft {
                                c.start.checked_sub(1)
                            } else {
                                Some(c.start + 1)
                            };
                            if let Some(to) = to {
                                self.move_column(id, c.start, to);
                            }
                        }
                    }
                }
            }
            Message::ColumnDialog(message) => {
                if let Some(Dialog::Column { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, ColumnMessage::Cancel);
                    if let Some(command) = dialog.update(message) {
                        let at = match &command {
                            Command::AddColumn { at, .. } => Some(*at),
                            _ => None,
                        };
                        if self.execute(id, command) {
                            self.dialog = None;
                            if let Some(at) = at {
                                self.select_columns(id, at..at + 1);
                            }
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Writer(message) => {
                if let Some(Dialog::Writer { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, WriterMessage::Cancel);
                    if let Some(command) = dialog.update(message) {
                        if self.execute(id, command) {
                            self.dialog = None;
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Metadata(message) => {
                if let Some(Dialog::Metadata { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, MetadataMessage::Cancel);
                    if let Some(command) = dialog.update(message) {
                        let result = self
                            .workbook
                            .get_mut(id)
                            .map(|doc| controller::execute(doc, command));
                        match result {
                            Some(Ok(())) => self.dialog = None,
                            Some(Err(e)) => dialog.set_error(e.to_string()),
                            None => {}
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Choose(message) => {
                if let Some(Dialog::Choose { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    match dialog.update(message.clone()) {
                        Some(Some(command)) => {
                            if self.execute(id, command) {
                                self.dialog = None;
                            }
                        }
                        Some(None) => self.dialog = None,
                        None if matches!(message, ChooseMessage::Cancel) => self.dialog = None,
                        None => {}
                    }
                }
            }
            Message::CloseMenu => self.menu = None,
            Message::Escape => {
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
                            self.config = config;
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

    fn perform(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Open => {
                return Task::perform(pick_files(), |files| {
                    Message::FilesPicked(files.unwrap_or_default())
                });
            }
            Action::Close => {
                if let Some(id) = self.active {
                    return self.request_close(vec![id], false);
                }
            }
            Action::Save => {
                if let Some(id) = self.active {
                    let has_path = self.workbook.get(id).and_then(Document::path).is_some();
                    return if has_path {
                        self.start_save(id, None)
                    } else {
                        self.save_as(id)
                    };
                }
            }
            Action::SaveAs => {
                if let Some(id) = self.active {
                    return self.save_as(id);
                }
            }
            Action::Undo | Action::Redo => {
                self.edit = None;
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get_mut(id)
                {
                    let result = if action == Action::Undo {
                        Ok(controller::undo(doc))
                    } else {
                        controller::redo(doc)
                    };
                    match result {
                        Ok(true) => self.refresh(id),
                        Ok(false) => {}
                        Err(e) => self.errors.push(e.to_string()),
                    }
                }
            }
            Action::InsertRows => {
                if let Some(id) = self.active {
                    self.insert_rows(id, false);
                }
            }
            Action::RemoveRows => {
                if let Some(id) = self.active {
                    self.delete_rows(id);
                }
            }
            Action::AddColumn => {
                if let Some(id) = self.active {
                    let at = self
                        .selected_columns(id)
                        .map_or_else(|| self.column_count(id), |c| c.end);
                    return self.open_column_dialog(id, ColumnDialog::add(at));
                }
            }
            Action::RenameColumn => {
                if let Some(id) = self.active {
                    return self.rename_column(id);
                }
            }
            Action::RemoveColumns => {
                if let Some(id) = self.active {
                    self.remove_columns(id);
                }
            }
            Action::ChooseColumns => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    let names = doc
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| f.name().clone())
                        .collect::<Vec<_>>();
                    self.dialog = Some(Dialog::Choose {
                        document: id,
                        dialog: ChooseColumns::new(names),
                    });
                }
            }
            Action::FileMetadata => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    self.dialog = Some(Dialog::Metadata {
                        document: id,
                        dialog: MetadataDialog::new(&doc.metadata().key_value),
                    });
                }
            }
            Action::WriterSettings => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    let columns = doc
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| (f.name().clone(), f.data_type().clone()))
                        .collect();
                    self.dialog = Some(Dialog::Writer {
                        document: id,
                        dialog: WriterDialog::new(doc.writer_settings().clone(), columns),
                    });
                }
            }
            Action::ToggleDetails => self.show_side_pane = !self.show_side_pane,
            Action::Mode(mode) => {
                self.config.appearance.mode = mode;
                self.save_config();
            }
            Action::Settings => self.dialog = Some(Dialog::Settings(Draft::new(&self.config))),
            Action::About => self.dialog = Some(Dialog::About),
            // Not built yet; the ribbon shows these disabled.
            _ => {}
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

    fn refresh(&mut self, id: DocumentId) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.refresh(doc);
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

    fn selected_rows(&self, id: DocumentId) -> Option<std::ops::Range<usize>> {
        self.tabs.iter().find(|t| t.id == id)?.grid.selected_rows()
    }

    /// Inserts as many empty rows as are selected, above or below the
    /// selection, and selects them.
    fn insert_rows(&mut self, id: DocumentId, below: bool) {
        self.commit_edit();
        let Some(rows) = self.selected_rows(id) else {
            return;
        };
        let at = if below { rows.end } else { rows.start };
        let count = rows.len();
        if self.execute(id, Command::InsertRows { at, count })
            && let (Some(doc), Some(tab)) = (
                self.workbook.get(id),
                self.tabs.iter_mut().find(|t| t.id == id),
            )
        {
            tab.grid.select_rows(at..at + count, doc);
        }
    }

    /// Closes documents, asking about unsaved changes first. With `exit`,
    /// quits the app afterwards.
    fn request_close(&mut self, documents: Vec<DocumentId>, exit: bool) -> Task<Message> {
        self.commit_edit();
        self.menu = None;
        self.closing = Some(Closing {
            queue: documents,
            exit,
        });
        self.continue_closing()
    }

    /// Closes queued documents until one has unsaved changes (then asks) or
    /// the queue is empty.
    fn continue_closing(&mut self) -> Task<Message> {
        loop {
            let Some(closing) = &mut self.closing else {
                return Task::none();
            };
            let exit = closing.exit;
            let Some(&id) = closing.queue.first() else {
                self.closing = None;
                self.dialog = None;
                return if exit { iced::exit() } else { Task::none() };
            };
            if self.workbook.get(id).is_some_and(Document::is_modified) {
                self.active = Some(id);
                self.dialog = Some(Dialog::ConfirmClose { document: id });
                return Task::none();
            }
            closing.queue.remove(0);
            if !exit {
                self.close(id);
            }
        }
    }

    fn answer_close(&mut self, answer: CloseAnswer) -> Task<Message> {
        let Some(Dialog::ConfirmClose { document }) = self.dialog else {
            return Task::none();
        };
        match answer {
            CloseAnswer::Cancel => {
                self.dialog = None;
                self.closing = None;
                Task::none()
            }
            CloseAnswer::Discard => {
                if let Some(closing) = &mut self.closing {
                    closing.queue.retain(|&id| id != document);
                    if !closing.exit {
                        self.close(document);
                    }
                }
                self.dialog = None;
                self.continue_closing()
            }
            CloseAnswer::Save => {
                self.dialog = None;
                // Keep it at the front of the queue: when the save finishes
                // it is no longer modified and gets closed.
                self.start_save(document, None)
            }
        }
    }

    /// Asks where to save, then saves there.
    fn save_as(&mut self, id: DocumentId) -> Task<Message> {
        self.commit_edit();
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let name = doc.title();
        let dir = doc.path().and_then(Path::parent).map(Path::to_path_buf);
        Task::perform(pick_save_path(name, dir), move |path| {
            Message::SaveTo(id, path)
        })
    }

    /// Saves on a worker thread; `None` saves to the document's own path.
    fn start_save(&mut self, id: DocumentId, target: Option<PathBuf>) -> Task<Message> {
        self.commit_edit();
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        match doc.save_job(target) {
            Ok(job) => {
                self.saving = Some(id);
                Task::perform(
                    blocking(move || job.run().map(Slot::new).map_err(|e| e.to_string())),
                    move |result| Message::Saved(id, result),
                )
            }
            Err(e) => {
                self.errors.push(format!("Could not save: {e}"));
                Task::none()
            }
        }
    }

    fn selected_columns(&self, id: DocumentId) -> Option<std::ops::Range<usize>> {
        self.tabs
            .iter()
            .find(|t| t.id == id)?
            .grid
            .selected_columns()
    }

    fn select_columns(&mut self, id: DocumentId, range: std::ops::Range<usize>) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.grid.select_columns(range);
        }
    }

    fn column_count(&self, id: DocumentId) -> usize {
        self.workbook.get(id).map_or(0, Document::num_columns)
    }

    fn column_names(&self, id: DocumentId, range: std::ops::Range<usize>) -> Vec<String> {
        let Some(doc) = self.workbook.get(id) else {
            return Vec::new();
        };
        let schema = doc.schema();
        schema.fields()
            [range.start.min(schema.fields().len())..range.end.min(schema.fields().len())]
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    fn open_column_dialog(&mut self, id: DocumentId, dialog: ColumnDialog) -> Task<Message> {
        self.edit = None;
        self.dialog = Some(Dialog::Column {
            document: id,
            dialog,
        });
        Task::batch([
            iced::widget::operation::focus(dialogs::NAME_ID),
            iced::widget::operation::select_all(dialogs::NAME_ID),
        ])
    }

    fn rename_column(&mut self, id: DocumentId) -> Task<Message> {
        let Some(columns) = self.selected_columns(id) else {
            return Task::none();
        };
        match self
            .column_names(id, columns.start..columns.start + 1)
            .pop()
        {
            Some(name) => self.open_column_dialog(id, ColumnDialog::rename(name)),
            None => Task::none(),
        }
    }

    fn remove_columns(&mut self, id: DocumentId) {
        self.edit = None;
        if let Some(columns) = self.selected_columns(id) {
            let names = self.column_names(id, columns);
            if !names.is_empty() {
                self.execute(id, Command::RemoveColumns { names });
            }
        }
    }

    fn move_column(&mut self, id: DocumentId, from: usize, to: usize) {
        self.commit_edit();
        if let Some(name) = self.column_names(id, from..from + 1).pop()
            && self.execute(id, Command::MoveColumn { name, to })
        {
            self.select_columns(id, to..to + 1);
        }
    }

    fn delete_rows(&mut self, id: DocumentId) {
        self.edit = None;
        if let Some(rows) = self.selected_rows(id) {
            self.execute(id, Command::DeleteRows { rows: vec![rows] });
        }
    }

    /// Row and column name of the selected cell in a document's grid.
    fn selected_cell(&self, id: DocumentId) -> Option<(usize, String)> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (row, column) = tab.grid.selected()?;
        let doc = self.workbook.get(id)?;
        Some((row, doc.schema().field(column).name().clone()))
    }

    /// Starts editing the selected cell in the formula bar.
    fn start_edit(&mut self, id: DocumentId, initial: Option<String>) -> Task<Message> {
        let Some((row, column)) = self.selected_cell(id) else {
            return Task::none();
        };
        let text = initial.unwrap_or_else(|| self.cell_text(id).unwrap_or_default());
        self.edit = Some(Edit {
            document: id,
            row,
            column,
            text,
        });
        Task::batch([
            iced::widget::operation::focus(FORMULA_ID),
            iced::widget::operation::move_cursor_to_end(FORMULA_ID),
        ])
    }

    /// Text of the selected cell (empty for null).
    fn cell_text(&self, id: DocumentId) -> Option<String> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (row, column) = tab.grid.selected()?;
        match tab.grid.value(row, column) {
            Some(value) => Some(value.unwrap_or_default().to_owned()),
            None => {
                let batch = self.workbook.get(id)?.read(row..row + 1).ok()?;
                let cells = veta_core::display::format_batch(&batch).ok()?;
                Some(cells.first()?.get(column)?.clone().unwrap_or_default())
            }
        }
    }

    /// Applies the formula bar edit, if any. Returns whether it was applied.
    fn commit_edit(&mut self) -> bool {
        let Some(edit) = self.edit.take() else {
            return false;
        };
        if self.cell_text(edit.document).as_deref() == Some(edit.text.as_str()) {
            return true;
        }
        let ok = self.execute(
            edit.document,
            Command::SetCell {
                row: edit.row,
                column: edit.column.clone(),
                value: Some(edit.text.clone()),
            },
        );
        if !ok {
            // Keep the text so it can be fixed.
            self.edit = Some(edit);
        }
        ok
    }

    fn save_config(&mut self) {
        if let Some(path) = &self.config_path
            && let Err(e) = self.config.save(path)
        {
            self.errors.push(format!("Could not save settings: {e}"));
        }
    }

    /// Starts opening `files` in the background. Files already open are
    /// focused instead.
    fn open(&mut self, files: Vec<PathBuf>) -> Task<Message> {
        let mut tasks = Vec::new();
        let options = OpenOptions {
            memory_budget: self.config.memory_budget(),
            force_paged: false,
        };
        for path in files {
            if let Some(id) = self.find_open(&path) {
                self.active = Some(id);
                continue;
            }
            if self.opening.contains(&path) {
                continue;
            }
            self.opening.push(path.clone());
            let worker_path = path.clone();
            tasks.push(Task::perform(
                blocking(move || {
                    Document::open(worker_path, options)
                        .map(Loaded::new)
                        .map_err(|e| e.to_string())
                }),
                move |result| Message::Opened(path.clone(), result),
            ));
        }
        Task::batch(tasks)
    }

    fn find_open(&self, path: &Path) -> Option<DocumentId> {
        let target = path.canonicalize().ok()?;
        self.workbook
            .iter()
            .find(|(_, doc)| {
                doc.path().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&target)
            })
            .map(|(id, _)| id)
    }

    fn close(&mut self, id: DocumentId) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        self.tabs.remove(index);
        self.workbook.close(id);
        if self.active == Some(id) {
            // Focus the tab that took its place, or the new last one.
            self.active = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(|t| t.id);
        }
    }

    fn active_document(&self) -> Option<(&Tab, &Document)> {
        let id = self.active?;
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        Some((tab, self.workbook.get(id)?))
    }

    fn view(&self) -> Element<'_, Message> {
        let ui = self.ui();
        let main: Element<'_, Message> = match self.active_document() {
            Some((tab, doc)) => {
                let id = tab.id;
                let edit = self.edit.as_ref().filter(|e| e.document == id);
                let grid = grid::grid(&tab.grid, ui.tokens, move |e| Message::Grid(id, e));
                let grid = column![
                    panes::formula_bar(tab, doc, edit, ui),
                    divider(ui.tokens, false),
                    container(grid).width(Length::Fill).height(Length::Fill),
                ];
                if self.show_side_pane {
                    row![grid, divider(ui.tokens, true), panes::side_pane(doc, ui)].into()
                } else {
                    grid.into()
                }
            }
            None => panes::empty_state(!self.opening.is_empty(), &self.config.recent_files, ui),
        };

        let context = ribbon::Context {
            has_document: self.active.is_some(),
            has_selection: self.active.and_then(|id| self.selected_rows(id)).is_some(),
            can_undo: self
                .active_document()
                .is_some_and(|(_, d)| d.history().can_undo()),
            can_redo: self
                .active_document()
                .is_some_and(|(_, d)| d.history().can_redo()),
            details_visible: self.show_side_pane,
            mode: self.config.appearance.mode,
            tokens: ui.tokens,
            text_size: ui.size,
        };
        let window = column![
            ribbon::view(&self.ribbon, context, Message::Ribbon, Message::Action),
            divider(ui.tokens, false),
            panes::errors(&self.errors, ui),
            container(main).height(Length::Fill),
            divider(ui.tokens, false),
            panes::tab_bar(
                self.tabs.iter().filter_map(|t| {
                    let doc = self.workbook.get(t.id)?;
                    let title = if doc.is_modified() {
                        format!("● {}", doc.title())
                    } else {
                        doc.title()
                    };
                    Some((t.id, title))
                }),
                self.active,
                ui,
            ),
            panes::status_bar(self.active_document(), self.opening.len(), ui),
        ];

        if self.saving.is_some() {
            let t = ui.tokens;
            return stack![
                window,
                opaque(
                    center(
                        container(iced::widget::text("Saving…").size(ui.heading()))
                            .padding(24)
                            .style(move |_theme: &Theme| {
                                container::Style::default()
                                    .background(t.background)
                                    .color(t.text)
                                    .border(iced::Border {
                                        color: t.border,
                                        width: 1.0,
                                        radius: 8.0.into(),
                                    })
                            })
                    )
                    .style(|_theme: &Theme| {
                        container::Style::default()
                            .background(Color::from_rgba(0.0, 0.0, 0.0, 0.25))
                    })
                ),
            ]
            .into();
        }
        if let Some(menu) = &self.menu {
            let rows = self.selected_rows(menu.document).map_or(1, |r| r.len());
            let columns = self.selected_columns(menu.document).map_or(1, |c| c.len());
            return stack![
                window,
                mouse_area(
                    container(iced::widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                )
                .on_press(Message::CloseMenu)
                .on_right_press(Message::CloseMenu),
                pin(panes::context_menu(menu.target, rows, columns, ui)).position(menu.position),
            ]
            .into();
        }
        let Some(dialog) = &self.dialog else {
            return window.into();
        };
        let card = match dialog {
            Dialog::Settings(draft) => {
                settings::view(draft, &self.themes, self.themes_dir.as_deref(), ui.tokens)
                    .map(Message::Settings)
            }
            Dialog::About => settings::about(Message::CloseDialog, ui.tokens),
            Dialog::Column { dialog, .. } => dialog.view(ui.tokens).map(Message::ColumnDialog),
            Dialog::Choose { dialog, .. } => dialog.view(ui.tokens).map(Message::Choose),
            Dialog::Writer { dialog, .. } => dialog.view(ui.tokens).map(Message::Writer),
            Dialog::Metadata { dialog, .. } => dialog.view(ui.tokens).map(Message::Metadata),
            Dialog::ConfirmClose { document } => {
                let name = self
                    .workbook
                    .get(*document)
                    .map(Document::title)
                    .unwrap_or_default();
                settings::confirm_close(name, ui.tokens)
            }
        };
        stack![
            window,
            opaque(
                mouse_area(center(opaque(card)).style(|_theme: &Theme| {
                    container::Style::default().background(Color::from_rgba(0.0, 0.0, 0.0, 0.35))
                }))
                .on_press(Message::CloseDialog)
            )
        ]
        .into()
    }
}

fn divider<'a>(tokens: Tokens, vertical: bool) -> Element<'a, Message> {
    let style = move |_theme: &Theme| rule::Style {
        color: tokens.border,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    };
    if vertical {
        rule::vertical(1).style(style).into()
    } else {
        rule::horizontal(1).style(style).into()
    }
}

async fn pick_save_path(name: String, dir: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Save as")
        .set_file_name(name)
        .add_filter("Parquet", &["parquet"]);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    let mut path = dialog.save_file().await?.path().to_path_buf();
    if path.extension().is_none() {
        path.set_extension("parquet");
    }
    Some(path)
}

async fn pick_files() -> Option<Vec<PathBuf>> {
    let handles = rfd::AsyncFileDialog::new()
        .set_title("Open Parquet files")
        .add_filter("Parquet", &["parquet", "parq", "pq"])
        .add_filter("All files", &["*"])
        .pick_files()
        .await?;
    Some(handles.iter().map(|h| h.path().to_path_buf()).collect())
}

/// Runs blocking work on its own thread so the UI stays responsive.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.await.expect("worker thread panicked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_testkit::{TempDir, fixtures};

    fn app() -> App {
        let (themes, _) = theme::load_all(None);
        let startup = Startup {
            config: Config::default(),
            config_path: None,
            themes,
            themes_dir: None,
            errors: Vec::new(),
        };
        App::new(startup, Vec::new()).0
    }

    fn app_with(paths: &[&Path]) -> App {
        let mut app = app();
        for path in paths {
            let doc = Document::open(path, OpenOptions::default()).unwrap();
            let _ = app.update(Message::Opened(path.to_path_buf(), Ok(Loaded::new(doc))));
        }
        app
    }

    #[test]
    fn opening_adds_and_focuses_tabs() {
        let dir = TempDir::new();
        let a = dir.join("a.parquet");
        let b = dir.join("b.parquet");
        fixtures::all_types(&a);
        fixtures::mixed_settings(&b);

        let app = app_with(&[&a, &b]);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, Some(app.tabs[1].id));
        assert_eq!(app.title(), "b.parquet — Veta");
        assert_eq!(app.config.recent_files.len(), 2);
    }

    #[test]
    fn opening_an_open_file_focuses_it() {
        let dir = TempDir::new();
        let a = dir.join("a.parquet");
        let b = dir.join("b.parquet");
        fixtures::all_types(&a);
        fixtures::all_types(&b);
        let mut app = app_with(&[&a, &b]);

        let _ = app.update(Message::FilesPicked(vec![a.clone()]));
        assert_eq!(app.tabs.len(), 2);
        assert!(app.opening.is_empty());
        assert_eq!(app.active, Some(app.tabs[0].id));
    }

    #[test]
    fn closing_focuses_neighbour() {
        let dir = TempDir::new();
        let paths: Vec<_> = (0..3).map(|i| dir.join(&format!("{i}.parquet"))).collect();
        for p in &paths {
            fixtures::all_types(p);
        }
        let mut app = app_with(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        let ids: Vec<_> = app.tabs.iter().map(|t| t.id).collect();

        let _ = app.update(Message::SelectTab(ids[1]));
        let _ = app.update(Message::Action(Action::Close));
        assert_eq!(app.active, Some(ids[2]));
        let _ = app.update(Message::CloseTab(ids[2]));
        assert_eq!(app.active, Some(ids[0]));
        let _ = app.update(Message::CloseTab(ids[0]));
        assert_eq!(app.active, None);
        assert!(app.workbook.is_empty());
    }

    #[test]
    fn open_failure_is_reported() {
        let mut app = app();
        app.opening.push("x.parquet".into());
        let _ = app.update(Message::Opened("x.parquet".into(), Err("boom".into())));
        assert!(app.opening.is_empty());
        assert_eq!(app.errors.len(), 1);
        let _ = app.update(Message::DismissError(0));
        assert!(app.errors.is_empty());
    }

    #[test]
    fn edit_commit_and_undo() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let score = |app: &App| app.tabs[0].grid.value(2, 2).flatten().map(str::to_owned);

        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 2, column: 2 }));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::StartEdit {
                initial: Some("7".into()),
            },
        ));
        let _ = app.update(Message::EditInput("7.25".into()));
        let _ = app.update(Message::CommitEdit);
        assert!(app.edit.is_none());
        assert_eq!(score(&app).as_deref(), Some("7.25"));
        assert_eq!(app.tabs[0].grid.selected(), Some((3, 2)), "moved down");
        assert!(app.workbook.get(id).unwrap().is_modified());

        // Invalid input keeps the edit open and reports the error.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::StartEdit {
                initial: Some("x".into()),
            },
        ));
        let _ = app.update(Message::CommitEdit);
        assert!(app.edit.is_some());
        assert_eq!(app.errors.len(), 1);
        let _ = app.update(Message::Escape);
        assert!(app.edit.is_none());

        let _ = app.update(Message::Action(Action::Undo));
        assert_eq!(score(&app).as_deref(), Some("1.0"));
        let _ = app.update(Message::Action(Action::Redo));
        assert_eq!(score(&app).as_deref(), Some("7.25"));
    }

    #[test]
    fn insert_and_delete_rows() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let rows = |app: &App| app.workbook.get(id).unwrap().num_rows();

        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectRows {
                row: 2,
                extend: false,
            },
        ));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectRows {
                row: 4,
                extend: true,
            },
        ));
        assert_eq!(app.selected_rows(id), Some(2..5));

        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Rows,
                position: iced::Point::ORIGIN,
            },
        ));
        assert!(app.menu.is_some());
        let _ = app.update(Message::Menu(MenuItem::InsertRowsBelow));
        assert!(app.menu.is_none());
        assert_eq!(rows(&app), 1003);
        assert_eq!(
            app.selected_rows(id),
            Some(5..8),
            "inserted rows are selected"
        );
        assert_eq!(app.tabs[0].grid.value(5, 0), Some(None));

        let _ = app.update(Message::Action(Action::RemoveRows));
        assert_eq!(rows(&app), 1000);
        // Like Excel, the same row positions stay selected after a delete.
        let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
        assert_eq!(rows(&app), 997);

        let _ = app.update(Message::Action(Action::Undo));
        let _ = app.update(Message::Action(Action::Undo));
        let _ = app.update(Message::Action(Action::Undo));
        assert_eq!(rows(&app), 1000);
        assert!(!app.workbook.get(id).unwrap().is_modified());
    }

    #[test]
    fn column_commands() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let names = |app: &App| -> Vec<String> {
            let doc = app.workbook.get(id).unwrap();
            doc.schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect()
        };

        // Insert a column right of "name" through the menu and dialog.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        app.menu = Some(ContextMenu {
            document: id,
            target: MenuTarget::Column(1),
            position: iced::Point::ORIGIN,
        });
        let _ = app.update(Message::Menu(MenuItem::InsertColumnRight));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("note".into())));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
        assert!(app.dialog.is_none());
        assert_eq!(names(&app), ["id", "name", "note", "score", "flag"]);
        assert_eq!(app.selected_columns(id), Some(2..3));

        // Rename with a clashing name keeps the dialog open.
        let _ = app.update(Message::Action(Action::RenameColumn));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("id".into())));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
        assert!(app.dialog.is_some());
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("memo".into())));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
        assert_eq!(names(&app), ["id", "name", "memo", "score", "flag"]);

        // Drag "memo" to the front, then remove two columns.
        let _ = app.update(Message::Grid(id, GridEvent::MoveColumn { from: 2, to: 0 }));
        assert_eq!(names(&app), ["memo", "id", "name", "score", "flag"]);
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 3,
                extend: true,
            },
        ));
        assert_eq!(app.selected_columns(id), Some(0..4));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 3,
                extend: false,
            },
        ));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 4,
                extend: true,
            },
        ));
        let _ = app.update(Message::Action(Action::RemoveColumns));
        assert_eq!(names(&app), ["memo", "id", "name"]);

        // Choose columns keeps only the checked ones.
        let _ = app.update(Message::Action(Action::ChooseColumns));
        let _ = app.update(Message::Choose(ChooseMessage::Toggle(0, false)));
        let _ = app.update(Message::Choose(ChooseMessage::Apply));
        assert_eq!(names(&app), ["id", "name"]);

        while app.workbook.get(id).unwrap().history().can_undo() {
            let _ = app.update(Message::Action(Action::Undo));
        }
        assert_eq!(names(&app), ["id", "name", "score", "flag"]);
    }

    #[test]
    fn save_in_place_and_save_as() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectRows {
                row: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::RemoveRows));
        assert!(app.workbook.get(id).unwrap().is_modified());

        // What the worker thread does, run inline.
        let run = |app: &App, target: Option<PathBuf>| {
            let job = app.workbook.get(id).unwrap().save_job(target).unwrap();
            Message::Saved(id, job.run().map(Slot::new).map_err(|e| e.to_string()))
        };
        app.saving = Some(id);
        let _ = app.update(Message::Action(Action::Undo)); // ignored while saving
        let saved = run(&app, None);
        let _ = app.update(saved);
        assert!(app.saving.is_none());
        let doc = app.workbook.get(id).unwrap();
        assert!(!doc.is_modified());
        assert_eq!(doc.num_rows(), 999);
        assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("1")));

        let copy = dir.join("copy.parquet");
        let saved = run(&app, Some(copy.clone()));
        let _ = app.update(saved);
        assert_eq!(app.title(), "copy.parquet — Veta");
        assert!(copy.exists());
    }

    #[test]
    fn closing_asks_about_unsaved_changes() {
        let dir = TempDir::new();
        let a = dir.join("a.parquet");
        let b = dir.join("b.parquet");
        fixtures::mixed_settings(&a);
        fixtures::mixed_settings(&b);
        let mut app = app_with(&[&a, &b]);
        let (ida, idb) = (app.tabs[0].id, app.tabs[1].id);

        // Unmodified tabs close straight away.
        let _ = app.update(Message::CloseTab(idb));
        assert_eq!(app.tabs.len(), 1);

        let _ = app.update(Message::Grid(
            ida,
            GridEvent::SelectRows {
                row: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::RemoveRows));
        assert!(app.title().starts_with('●'));

        let _ = app.update(Message::Action(Action::Close));
        assert!(matches!(app.dialog, Some(Dialog::ConfirmClose { .. })));
        let _ = app.update(Message::CloseAnswer(CloseAnswer::Cancel));
        assert!(app.dialog.is_none() && app.closing.is_none());
        assert_eq!(app.tabs.len(), 1);

        let _ = app.update(Message::Action(Action::Close));
        let _ = app.update(Message::CloseAnswer(CloseAnswer::Discard));
        assert!(app.tabs.is_empty());
        assert!(app.dialog.is_none() && app.closing.is_none());
    }

    #[test]
    fn save_then_close() {
        let dir = TempDir::new();
        let a = dir.join("a.parquet");
        fixtures::mixed_settings(&a);
        let mut app = app_with(&[&a]);
        let id = app.tabs[0].id;
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectRows {
                row: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::RemoveRows));

        let _ = app.update(Message::CloseTab(id));
        let _ = app.update(Message::CloseAnswer(CloseAnswer::Save));
        assert_eq!(app.saving, Some(id));
        // What the worker thread does.
        let job = app.workbook.get(id).unwrap().save_job(None).unwrap();
        let _ = app.update(Message::Saved(
            id,
            job.run().map(Slot::new).map_err(|e| e.to_string()),
        ));
        assert!(app.tabs.is_empty(), "closed after saving");
        assert_eq!(
            Document::open(&a, OpenOptions::default())
                .unwrap()
                .num_rows(),
            999
        );
    }

    #[test]
    fn quitting_goes_through_every_modified_document() {
        let dir = TempDir::new();
        let paths: Vec<_> = (0..3).map(|i| dir.join(&format!("{i}.parquet"))).collect();
        for p in &paths {
            fixtures::mixed_settings(p);
        }
        let mut app = app_with(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        for id in [app.tabs[0].id, app.tabs[2].id] {
            let _ = app.update(Message::Grid(
                id,
                GridEvent::SelectRows {
                    row: 0,
                    extend: false,
                },
            ));
            let _ = app.update(Message::SelectTab(id));
            let _ = app.update(Message::Action(Action::RemoveRows));
        }
        let _ = app.update(Message::CloseRequested);
        assert!(
            matches!(app.dialog, Some(Dialog::ConfirmClose { document }) if document == app.tabs[0].id)
        );
        let _ = app.update(Message::CloseAnswer(CloseAnswer::Discard));
        assert!(
            matches!(app.dialog, Some(Dialog::ConfirmClose { document }) if document == app.tabs[2].id)
        );
        let _ = app.update(Message::CloseAnswer(CloseAnswer::Discard));
        assert!(app.dialog.is_none() && app.closing.is_none(), "exits");
    }

    #[test]
    fn theme_follows_mode() {
        let mut app = app();
        assert_eq!(app.current_theme().name, theme::DEFAULT_LIGHT);
        let _ = app.update(Message::SystemMode(iced::theme::Mode::Dark));
        assert_eq!(app.current_theme().name, theme::DEFAULT_DARK);
        let _ = app.update(Message::Action(Action::Mode(ModePreference::Light)));
        assert_eq!(app.current_theme().name, theme::DEFAULT_LIGHT);
    }

    #[test]
    fn missing_theme_falls_back_to_built_in() {
        let (themes, _) = theme::load_all(None);
        let mut config = Config::default();
        config.appearance.mode = ModePreference::Dark;
        config.appearance.dark_theme = "Gone".into();
        assert_eq!(pick_theme(&themes, &config, None).name, theme::DEFAULT_DARK);
    }

    #[test]
    fn settings_dialog_saves_config() {
        let mut app = app();
        let _ = app.update(Message::Action(Action::Settings));
        assert!(matches!(app.dialog, Some(Dialog::Settings(_))));
        let _ = app.update(Message::Settings(SettingsMessage::Mode(
            ModePreference::Dark,
        )));
        let _ = app.update(Message::Settings(SettingsMessage::Save));
        assert!(app.dialog.is_none());
        assert_eq!(app.config.appearance.mode, ModePreference::Dark);

        let _ = app.update(Message::Action(Action::About));
        let _ = app.update(Message::CloseDialog);
        assert!(app.dialog.is_none());
    }
}
