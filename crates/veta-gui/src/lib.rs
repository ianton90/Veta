//! Veta's desktop interface, built with iced.
//!
//! The iced state holds the core [`Workbook`] (the model) plus GUI-only state
//! (tabs, grid scroll positions, panes, settings). Messages that change data
//! will be turned into core commands; see `docs/ARCHITECTURE.md`.

mod config;
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
}

#[derive(Debug, Clone)]
enum Message {
    Action(Action),
    Ribbon(RibbonMessage),
    FilesPicked(Vec<PathBuf>),
    Opened(PathBuf, Result<Loaded, String>),
    SelectTab(DocumentId),
    CloseTab(DocumentId),
    Grid(DocumentId, GridEvent),
    DismissError(usize),
    Settings(SettingsMessage),
    CloseDialog,
    /// Esc: closes a dialog or cancels an edit.
    Escape,
    EditInput(String),
    CommitEdit,
    Menu(MenuItem),
    CloseMenu,
    SystemMode(iced::theme::Mode),
    OpenDialog,
}

/// A document opened on a worker thread. Messages must be `Clone`, documents
/// are not, so the document travels in a take-once slot.
#[derive(Debug, Clone)]
struct Loaded(Arc<Mutex<Option<Document>>>);

impl Loaded {
    fn new(document: Document) -> Self {
        Self(Arc::new(Mutex::new(Some(document))))
    }

    fn take(&self) -> Option<Document> {
        self.0.lock().ok()?.take()
    }
}

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
        };
        let open = app.open(files);
        let system = iced::system::theme().map(Message::SystemMode);
        (app, Task::batch([open, system]))
    }

    fn title(&self) -> String {
        match self.active_document() {
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
            iced::system::theme_changes().map(Message::SystemMode),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
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
            Message::SelectTab(id) => self.active = Some(id),
            Message::CloseTab(id) => self.close(id),
            Message::Grid(id, event) => match event {
                GridEvent::StartEdit { initial } => return self.start_edit(id, initial),
                GridEvent::InsertRows => self.insert_rows(id, false),
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
                }
            }
            Message::CloseMenu => self.menu = None,
            Message::Escape => {
                if self.menu.take().is_some() {
                } else if self.dialog.is_some() {
                    self.dialog = None;
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
            Message::CloseDialog => self.dialog = None,
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
                    self.close(id);
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
                self.tabs
                    .iter()
                    .filter_map(|t| Some((t.id, self.workbook.get(t.id)?.title()))),
                self.active,
                ui,
            ),
            panes::status_bar(self.active_document(), self.opening.len(), ui),
        ];

        if let Some(menu) = &self.menu {
            let rows = self.selected_rows(menu.document).map_or(1, |r| r.len());
            return stack![
                window,
                mouse_area(
                    container(iced::widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                )
                .on_press(Message::CloseMenu)
                .on_right_press(Message::CloseMenu),
                pin(panes::context_menu(menu.target, rows, ui)).position(menu.position),
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
