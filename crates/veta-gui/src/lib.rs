//! Veta's desktop interface, built with iced.
//!
//! The iced state holds the core [`Workbook`] (the model) plus GUI-only state
//! (tabs, grid scroll positions, panes, settings). Messages that change data
//! will be turned into core commands; see `docs/ARCHITECTURE.md`.

mod config;
mod dialogs;
mod files;
mod grid;
mod icon;
mod instance;
mod panes;
mod ribbon;
mod settings;
mod theme;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use iced::widget::{center, column, container, mouse_area, opaque, pin, row, rule, stack};
use iced::{
    Color, Element, Font, Length, Pixels, Size, Subscription, Task, Theme, event, font, keyboard,
    window,
};
use veta_core::arrow::array::ArrayRef;
use veta_core::convert::ImportReport;
use veta_core::stats::ColumnStats;
use veta_core::steps::{
    Cast, ComputeJob, Computed, Condition, CustomColumn, Fill, Filter, FilterOp, Merge, OnError,
    Replace, Sort, SortKey, Split, Status, Step, TextOp,
};
use veta_core::{Command, Document, DocumentId, OpenOptions, Workbook, controller};

use crate::config::Config;
#[cfg(test)]
use crate::config::ModePreference;
use crate::dialogs::{
    CastDialog, CastMessage, ChooseColumns, ChooseMessage, ColumnDialog, ColumnMessage,
    CombineDialog, CombineMessage, CustomDialog, CustomMessage, ExportDialog, ExportMessage,
    FilterDialog, FilterMessage, FormatDialog, FormatMessage, ImportDialog, ImportMessage,
    MergeDialog, MergeMessage, MetadataDialog, MetadataMessage, ReplaceDialog, ReplaceMessage,
    SortDialog, SortMessage, SplitDialog, SplitMessage, WriterDialog, WriterMessage,
};
use crate::files::FileKind;
use crate::grid::{GRID_ID, GridEvent, GridView, MenuTarget, Nav, Selection};
use crate::ribbon::{Action, Ribbon, RibbonMessage};
use crate::settings::{Draft, Outcome, SettingsMessage};
use crate::theme::{Mode, Tokens, VetaTheme};

/// Id of the formula bar's text input.
const FORMULA_ID: iced::widget::Id = iced::widget::Id::new("formula");
/// Id of the input editing a value in the Value tab.
const LEAF_ID: iced::widget::Id = iced::widget::Id::new("leaf-editor");
/// Id of the text input drawn inside the cell being edited.
const EDITOR_ID: iced::widget::Id = iced::widget::Id::new("cell-editor");
/// Rows dialogs sample (to preview cast failures, suggest split parts).
const SAMPLE_ROWS: usize = 1000;

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
fn nested_columns(schema: &veta_core::arrow::datatypes::Schema) -> Vec<String> {
    schema
        .fields()
        .iter()
        .filter(|f| f.data_type().is_nested())
        .map(|f| f.name().clone())
        .collect()
}

/// A background import or export, shown in the status bar.
#[derive(Debug)]
pub struct Transfer {
    pub id: u64,
    /// The file being imported, if this is an import.
    importing: Option<PathBuf>,
    pub label: String,
    pub progress: f32,
    cancel: Arc<AtomicBool>,
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether an error message is the core's "cancelled".
fn is_cancelled(error: &str) -> bool {
    error == veta_core::Error::Cancelled.to_string()
}

/// Each top-level column with its metadata: as edited in Veta, otherwise
/// the column's own (sorted by key).
fn column_metadata(doc: &Document) -> Vec<(String, Vec<veta_core::model::KeyValue>)> {
    doc.schema()
        .fields()
        .iter()
        .map(|field| {
            let entries = match doc.metadata().column(field.name()) {
                Some(entries) => entries.to_vec(),
                None => {
                    let mut entries: Vec<_> = field
                        .metadata()
                        .iter()
                        .map(|(k, v)| veta_core::model::KeyValue {
                            key: k.clone(),
                            value: Some(v.clone()),
                        })
                        .collect();
                    entries.sort_by(|a, b| a.key.cmp(&b.key));
                    entries
                }
            };
            (field.name().clone(), entries)
        })
        .collect()
}

/// The app icon as 64×64 RGBA, rendered by `packaging/icons/render.sh`.
const APP_ICON: &[u8] = include_bytes!("../../../packaging/icons/veta-64.rgba");

fn window_settings() -> window::Settings {
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
    /// Files from other launches, opened once no save is running.
    queued_files: Vec<PathBuf>,
    /// Size of the window, to keep menus on screen.
    window_size: Size,
    /// GUI state per open document, in tab order.
    tabs: Vec<Tab>,
    active: Option<DocumentId>,
    show_side_pane: bool,
    side_tab: SideTab,
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
    /// The full pass running in the background, if any.
    computing: Option<Computing>,
    /// Column statistics for the side pane.
    stats: Stats,
    /// Nodes collapsed in the value tree, by path.
    collapsed: HashSet<String>,
    /// A value inside a nested cell being edited in the Value tab.
    leaf_edit: Option<LeafEdit>,
    /// Exports running in the background.
    /// Imports and exports running in the background.
    transfers: Vec<Transfer>,
    next_transfer: u64,
    /// Last thing done worth telling (shown in the status bar).
    notice: Option<String>,
    /// A "remove the error column if empty" step just added after a cast,
    /// to drop again (keeping the errors) if the column isn't empty.
    error_check: Option<(DocumentId, usize)>,
}

/// A step's full pass running on a worker thread.
#[derive(Debug)]
struct Computing {
    document: DocumentId,
    target: Target,
    /// The job's key; it is stale once the document's key differs.
    key: Vec<u64>,
    step: usize,
    progress: f32,
    cancel: Arc<AtomicBool>,
}

/// A value inside a nested cell, edited from the Value tab.
#[derive(Debug, Clone, PartialEq)]
struct LeafEdit {
    document: DocumentId,
    row: usize,
    column: usize,
    pointer: Vec<veta_core::display::PathPart>,
    data_type: veta_core::arrow::datatypes::DataType,
    text: String,
    error: Option<String>,
    /// Renaming the map key at `pointer` instead of editing the value.
    rename: bool,
}

/// Statistics computed for the side pane, and the pass running.
#[derive(Debug, Default)]
struct Stats {
    /// Results by document, data revision and column.
    cache: HashMap<(DocumentId, u64, String), ColumnStats>,
    running: Option<StatsRun>,
}

#[derive(Debug)]
struct StatsRun {
    document: DocumentId,
    revision: u64,
    columns: Vec<String>,
    progress: f32,
    cancel: Arc<AtomicBool>,
}

/// What a background pass computes for a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A step of the document.
    Document,
    /// The tab's temporary filter.
    View,
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
    Cut,
    Copy,
    Paste,
    EditCell,
    ClearCell,
    InsertRowsAbove,
    InsertRowsBelow,
    /// Appends a row (blank area menu).
    AddRow,
    /// Adds a column at the end (blank area menu).
    AddColumn,
    KeepRows,
    DeleteRows,
    InsertColumnLeft,
    InsertColumnRight,
    RenameColumn,
    RemoveColumns,
    KeepColumns,
    MoveColumnLeft,
    MoveColumnRight,
    FilterColumn,
    ChangeType,
    ReplaceValues,
    FillDown,
    FillUp,
    FormatText,
    SplitColumn,
    MergeColumns,
    DuplicateColumn,
    ExpandStruct,
    CombineStruct,
    /// Expand a struct column and the structs inside it.
    ExpandAll,
    ExplodeList,
    /// Open the cell in the JSON editor.
    EditJson,
    SortAscending,
    SortDescending,
    /// Filter to rows whose value in this column equals the selected cell's.
    KeepValue,
    /// Filter out rows whose value in this column equals the selected cell's.
    ExcludeValue,
}

#[derive(Debug, Clone, PartialEq)]
struct Edit {
    document: DocumentId,
    row: usize,
    column: String,
    /// The struct field edited, when the cell is a field of an expanded
    /// struct column.
    field: Option<StructField>,
    text: String,
    /// Why the last attempt to apply it failed.
    error: Option<String>,
}

/// A field inside a struct cell: the field names down to it and its type.
#[derive(Debug, Clone, PartialEq)]
struct StructField {
    path: Vec<String>,
    data_type: veta_core::arrow::datatypes::DataType,
}

/// What the side pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SideTab {
    #[default]
    Steps,
    Stats,
    /// The selected cell's value, as a tree for nested values.
    Value,
    File,
}

/// Whether the step settings dialog exists for this kind of step.
fn step_dialog_supported(step: &Step) -> bool {
    matches!(
        step,
        Step::RenameColumn { .. }
            | Step::AddColumn { .. }
            | Step::Filter(_)
            | Step::Sort(_)
            | Step::Cast(_)
            | Step::Replace(_)
            | Step::FormatText { .. }
            | Step::Split(_)
            | Step::Merge(_)
            | Step::CombineStruct { .. }
            | Step::CustomColumn(_)
    )
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
        /// Editing the step at this index instead of adding a new one.
        replace: Option<usize>,
    },
    Filter {
        document: DocumentId,
        dialog: FilterDialog,
        /// Editing the step at this index instead of adding a new one.
        replace: Option<usize>,
    },
    Cast {
        document: DocumentId,
        dialog: CastDialog,
        /// Editing the step at this index instead of adding a new one.
        replace: Option<usize>,
    },
    Replace {
        document: DocumentId,
        dialog: ReplaceDialog,
        replace: Option<usize>,
    },
    Format {
        document: DocumentId,
        dialog: FormatDialog,
        replace: Option<usize>,
    },
    Split {
        document: DocumentId,
        dialog: SplitDialog,
        replace: Option<usize>,
    },
    Merge {
        document: DocumentId,
        dialog: MergeDialog,
        replace: Option<usize>,
    },
    Combine {
        document: DocumentId,
        dialog: CombineDialog,
        replace: Option<usize>,
    },
    Custom {
        document: DocumentId,
        dialog: CustomDialog,
        replace: Option<usize>,
    },
    Import(ImportDialog),
    Export {
        document: DocumentId,
        dialog: ExportDialog,
    },
    Sort {
        document: DocumentId,
        dialog: SortDialog,
        /// Editing the step at this index instead of adding a new one.
        replace: Option<usize>,
    },
    /// "Later steps refer to rows by position" before changing a step.
    ConfirmStep {
        document: DocumentId,
        command: Command,
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
    Json {
        document: DocumentId,
        dialog: dialogs::JsonDialog,
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
    /// Files sent by another `veta` launch.
    Remote(Vec<PathBuf>),
    WindowResized(Size),
    Opened(PathBuf, Result<Loaded, String>),
    /// A CSV import finished, with what it found.
    Imported(PathBuf, Result<Slot<(Document, ImportReport)>, String>),
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
    Filter(FilterMessage),
    Sort(SortMessage),
    Cast(CastMessage),
    Replace(ReplaceMessage),
    Format(FormatMessage),
    Split(SplitMessage),
    Custom(CustomMessage),
    Merge(MergeMessage),
    Combine(CombineMessage),
    /// Progress (0–1) of the background pass with this key.
    ComputeProgress(DocumentId, Target, Vec<u64>, f32),
    /// The background pass with this key finished.
    ComputeDone(DocumentId, Target, Vec<u64>, Result<Slot<Computed>, String>),
    /// Stop the background pass and undo the change that started it.
    CancelCompute,
    SideTab(SideTab),
    /// Show the data after this many steps (past the last: all steps).
    PreviewStep(DocumentId, usize),
    RemoveStep(usize),
    MoveStep(usize, usize),
    EditStep(usize),
    ConfirmStep(bool),
    /// Save the active document's steps as a recipe.
    StatsProgress(DocumentId, u64, f32),
    /// Start editing a value inside the selected nested cell.
    EditLeaf(
        Vec<veta_core::display::PathPart>,
        veta_core::arrow::datatypes::DataType,
        String,
    ),
    /// Start renaming the map key at the pointer.
    EditKey(Vec<veta_core::display::PathPart>, String),
    /// Add or remove items and entries inside the selected nested cell.
    NestedEdit(
        Vec<veta_core::display::PathPart>,
        veta_core::values::NestedEdit,
    ),
    LeafInput(String),
    CommitLeaf,
    /// Open the selected nested cell in the JSON editor.
    OpenJsonEditor,
    /// Clipboard text read for pasting.
    Pasted(Option<String>),
    Json(dialogs::JsonMessage),
    /// Collapse or expand a node of the value tree.
    ToggleNode(String),
    StatsDone(DocumentId, u64, Result<Vec<ColumnStats>, String>),
    /// A file to import was chosen.
    ImportPicked(Option<PathBuf>),
    Import(ImportMessage),
    Export(ExportMessage),
    /// Where to export was chosen.
    ExportTo(DocumentId, Option<PathBuf>),
    Exported(Result<PathBuf, String>),
    /// A background import or export reached this fraction.
    TransferProgress(u64, f32),
    TransferDone(u64),
    CancelTransfer(u64),
    /// Turn off the temporary row filter.
    ClearFilter,
    SaveRecipe,
    SaveRecipeTo(DocumentId, Option<PathBuf>),
    /// Append a recipe's steps to the active document.
    LoadRecipe,
    LoadRecipeFrom(DocumentId, Option<PathBuf>),
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
            collapsed: HashSet::new(),
            leaf_edit: None,
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
        Task::batch([task, queued, self.schedule_compute(), self.schedule_stats()])
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
            Message::Action(action) => {
                self.notice = None;
                return self.perform(action);
            }
            Message::OpenDialog => return self.perform(Action::Open),
            Message::Ribbon(message) => self.ribbon.update(message),
            Message::FilesPicked(files) => return self.open(files),
            Message::WindowResized(size) => self.window_size = size,
            Message::Remote(files) => {
                self.queued_files.extend(files);
                return window::latest().and_then(window::gain_focus);
            }
            Message::Opened(path, result) => {
                self.opening.retain(|p| p != &path);
                match result.map(|loaded| loaded.take()) {
                    Ok(Some(document)) => self.add_document(&path, document),
                    Ok(None) => {}
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => self
                        .errors
                        .push(format!("Could not open {}: {e}", path.display())),
                }
            }
            Message::Imported(path, result) => {
                self.opening.retain(|p| p != &path);
                match result.map(|slot| slot.take()) {
                    Ok(Some((document, report))) => {
                        self.add_document(&path, document);
                        if let Some(summary) = report.summary() {
                            self.errors.push(summary);
                        }
                    }
                    Ok(None) => {}
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => self
                        .errors
                        .push(format!("Could not import {}: {e}", path.display())),
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
            Message::EditInput(text) => match &mut self.edit {
                Some(edit) => {
                    edit.text = text;
                    edit.error = None;
                }
                None => {
                    // Typing straight into the formula bar starts an edit.
                    if let Some(id) = self.active
                        && self.editable(id)
                    {
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
                let views_only = matches!(
                    item,
                    MenuItem::KeepValue
                        | MenuItem::ExcludeValue
                        | MenuItem::FilterColumn
                        | MenuItem::Copy
                );
                if !views_only && !self.editable(id) {
                    self.not_editable(id);
                    return Task::none();
                }
                match item {
                    MenuItem::KeepRows => self.keep_rows(id),
                    MenuItem::EditCell => return self.start_edit(id, None),
                    MenuItem::ClearCell => {
                        return self.update(Message::Grid(id, GridEvent::ClearCell));
                    }
                    MenuItem::InsertRowsAbove => self.insert_rows(id, false),
                    MenuItem::InsertRowsBelow => self.insert_rows(id, true),
                    MenuItem::AddRow => self.append_row(id),
                    MenuItem::AddColumn => {
                        let at = self.workbook.get(id).map_or(0, Document::num_columns);
                        return self.open_column_dialog(id, ColumnDialog::add(at));
                    }
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
                    MenuItem::RemoveColumns => self.keep_or_remove_columns(id, false, true),
                    MenuItem::KeepColumns => self.keep_or_remove_columns(id, true, true),
                    MenuItem::FilterColumn => return self.open_filter_dialog(id),
                    MenuItem::ChangeType => self.open_cast_dialog(id, None),
                    MenuItem::ReplaceValues => return self.open_replace_dialog(id, None),
                    MenuItem::FillDown | MenuItem::FillUp => {
                        self.fill_selected(id, item == MenuItem::FillUp);
                    }
                    MenuItem::FormatText => self.open_format_dialog(id),
                    MenuItem::SplitColumn => return self.open_split_dialog(id, None),
                    MenuItem::MergeColumns => return self.open_merge_dialog(id, None),
                    MenuItem::CombineStruct => return self.open_combine_dialog(id, None),
                    MenuItem::DuplicateColumn => self.duplicate_column(id),
                    MenuItem::Cut => return self.copy_selection(id, true),
                    MenuItem::Copy => return self.copy_selection(id, false),
                    MenuItem::Paste => return iced::clipboard::read().map(Message::Pasted),
                    MenuItem::ExpandStruct => self.nested_step(id, false, false),
                    MenuItem::ExpandAll => self.nested_step(id, false, true),
                    MenuItem::ExplodeList => self.nested_step(id, true, false),
                    MenuItem::EditJson => self.open_json_editor(id),
                    MenuItem::SortAscending | MenuItem::SortDescending => {
                        self.sort_selected(id, item == MenuItem::SortDescending);
                    }
                    MenuItem::KeepValue | MenuItem::ExcludeValue => {
                        self.quick_filter(id, item == MenuItem::KeepValue);
                    }
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
                if let Some(Dialog::Column {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let id = *document;
                    let cancel = matches!(message, ColumnMessage::Cancel);
                    let replace = *replace;
                    if let (Some(index), Some(command)) = (replace, dialog.update(message.clone()))
                    {
                        let step = match command {
                            Command::AddColumn {
                                name,
                                data_type,
                                at,
                            } => Step::AddColumn {
                                name,
                                data_type,
                                at,
                            },
                            Command::RenameColumn { from, to } => Step::RenameColumn { from, to },
                            _ => return Task::none(),
                        };
                        self.dialog = None;
                        self.change_steps(id, index, Command::ReplaceStep { index, step });
                    } else if replace.is_some() {
                        if cancel {
                            self.dialog = None;
                        }
                    } else if let Some(command) = dialog.update(message) {
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
            Message::Filter(message) => {
                if let Some(Dialog::Filter {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let id = *document;
                    let replace = *replace;
                    let cancel = matches!(message, FilterMessage::Cancel);
                    match (dialog.update(message), replace) {
                        (Some(filter), Some(index)) => {
                            self.dialog = None;
                            let step = Step::Filter(filter);
                            self.change_steps(id, index, Command::ReplaceStep { index, step });
                        }
                        (Some(filter), None) => match self.set_view_filter(id, filter) {
                            Ok(()) => self.dialog = None,
                            Err(e) => {
                                if let Some(Dialog::Filter { dialog, .. }) = &mut self.dialog {
                                    dialog.set_error(e);
                                }
                            }
                        },
                        (None, _) if cancel => self.dialog = None,
                        (None, _) => {}
                    }
                }
            }
            Message::Cast(message) => {
                if let Some(Dialog::Cast {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, CastMessage::Cancel);
                    match (dialog.update(message), replace) {
                        (Some(cast), Some(index)) => {
                            self.dialog = None;
                            let step = Step::Cast(cast);
                            self.change_steps(id, index, Command::ReplaceStep { index, step });
                        }
                        (Some(cast), None) => {
                            // With an error column, a second step removes
                            // it if nothing failed (checked on every row).
                            let check = (cast.options.on_error == OnError::ErrorColumn)
                                .then(|| cast.error_column());
                            let command = match &check {
                                Some(name) => Command::AppendSteps {
                                    steps: vec![
                                        Step::Cast(cast),
                                        Step::RemoveEmptyColumn { name: name.clone() },
                                    ],
                                },
                                None => Command::Cast(cast),
                            };
                            if self.execute(id, command) {
                                self.dialog = None;
                                if check.is_some() {
                                    let index = self
                                        .workbook
                                        .get(id)
                                        .map_or(0, |d| d.steps().len().saturating_sub(1));
                                    self.error_check = Some((id, index));
                                }
                            }
                        }
                        (None, _) if cancel => self.dialog = None,
                        (None, _) => {}
                    }
                }
            }
            Message::Replace(message) => {
                if let Some(Dialog::Replace {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, ReplaceMessage::Cancel);
                    match (dialog.update(message), replace) {
                        (Some(r), Some(index)) => {
                            self.dialog = None;
                            let step = Step::Replace(r);
                            self.change_steps(id, index, Command::ReplaceStep { index, step });
                        }
                        (Some(r), None) => {
                            let result = self
                                .workbook
                                .get_mut(id)
                                .map(|doc| controller::execute(doc, Command::Replace(r)));
                            match result {
                                Some(Ok(())) => {
                                    self.dialog = None;
                                    self.refresh(id);
                                }
                                Some(Err(e)) => dialog.set_error(e.to_string()),
                                None => {}
                            }
                        }
                        (None, _) if cancel => self.dialog = None,
                        (None, _) => {}
                    }
                }
            }
            Message::Format(message) => {
                if let Some(Dialog::Format {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, FormatMessage::Cancel);
                    match (dialog.update(message), replace) {
                        (Some((columns, op)), Some(index)) => {
                            self.dialog = None;
                            let step = Step::FormatText { columns, op };
                            self.change_steps(id, index, Command::ReplaceStep { index, step });
                        }
                        (Some((columns, op)), None) => {
                            self.dialog = None;
                            self.execute(id, Command::FormatText { columns, op });
                        }
                        (None, _) if cancel => self.dialog = None,
                        (None, _) => {}
                    }
                }
            }
            Message::Split(message) => {
                if let Some(Dialog::Split {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, SplitMessage::Cancel);
                    if let Some(split) = dialog.update(message) {
                        let command = Command::Split(split.clone());
                        if let Err(e) =
                            self.add_or_replace(id, replace, Step::Split(split), command)
                            && let Some(Dialog::Split { dialog, .. }) = &mut self.dialog
                        {
                            dialog.set_error(e);
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Custom(message) => {
                if let Some(Dialog::Custom {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, CustomMessage::Cancel);
                    if let Some(custom) = dialog.update(message) {
                        let command = Command::CustomColumn(custom.clone());
                        let step = Step::CustomColumn(custom);
                        if let Err(e) = self.add_or_replace(id, replace, step, command)
                            && let Some(Dialog::Custom { dialog, .. }) = &mut self.dialog
                        {
                            dialog.set_error(e);
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Merge(message) => {
                if let Some(Dialog::Merge {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, MergeMessage::Cancel);
                    if let Some(merge) = dialog.update(message) {
                        let command = Command::Merge(merge.clone());
                        if let Err(e) =
                            self.add_or_replace(id, replace, Step::Merge(merge), command)
                            && let Some(Dialog::Merge { dialog, .. }) = &mut self.dialog
                        {
                            dialog.set_error(e);
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Combine(message) => {
                if let Some(Dialog::Combine {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, CombineMessage::Cancel);
                    if let Some((columns, name)) = dialog.update(message) {
                        let step = Step::CombineStruct {
                            columns: columns.clone(),
                            name: name.clone(),
                        };
                        let command = Command::CombineStruct { columns, name };
                        if let Err(e) = self.add_or_replace(id, replace, step, command)
                            && let Some(Dialog::Combine { dialog, .. }) = &mut self.dialog
                        {
                            dialog.set_error(e);
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Sort(message) => {
                if let Some(Dialog::Sort {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = matches!(message, SortMessage::Cancel);
                    if let Some(sort) = dialog.update(message) {
                        let command = Command::Sort(sort.clone());
                        if let Err(e) = self.add_or_replace(id, replace, Step::Sort(sort), command)
                        {
                            self.errors.push(e);
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::ComputeProgress(id, target, key, progress) => {
                if let Some(c) = &mut self.computing
                    && c.document == id
                    && c.target == target
                    && c.key == key
                {
                    c.progress = progress;
                }
            }
            Message::ComputeDone(id, target, key, result) => {
                if self
                    .computing
                    .as_ref()
                    .is_some_and(|c| c.document == id && c.target == target && c.key == key)
                {
                    self.computing = None;
                }
                if target == Target::View {
                    if let (Some(doc), Some(tab)) = (
                        self.workbook.get(id),
                        self.tabs.iter_mut().find(|t| t.id == id),
                    ) {
                        match result.map(|slot| slot.take()) {
                            Ok(Some(computed)) => {
                                tab.grid.install_filter(computed, doc);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tab.grid.set_filter(None, doc);
                                self.errors.push(format!("Could not filter: {e}"));
                            }
                        }
                    }
                    return Task::none();
                }
                let Some(doc) = self.workbook.get_mut(id) else {
                    return Task::none();
                };
                let changed = match result.map(|slot| slot.take()) {
                    Ok(Some(computed)) => doc.install(computed),
                    Ok(None) => false,
                    Err(e) => doc.fail_compute(&key, e),
                };
                if changed {
                    self.after_compute(id);
                    self.refresh(id);
                }
            }
            Message::CancelCompute => {
                if let Some(c) = self.computing.take() {
                    c.cancel.store(true, Ordering::Relaxed);
                    if c.target == Target::View {
                        self.clear_filter(c.document);
                    } else if let Some(doc) = self.workbook.get_mut(c.document) {
                        if !controller::undo(doc) {
                            doc.fail_compute(&c.key, "Cancelled".into());
                        }
                        self.refresh(c.document);
                    }
                }
            }
            Message::SideTab(side) => self.side_tab = side,
            Message::PreviewStep(id, level) => {
                self.commit_edit_or_drop();
                if let (Some(doc), Some(tab)) = (
                    self.workbook.get(id),
                    self.tabs.iter_mut().find(|t| t.id == id),
                ) {
                    let preview = (level < doc.evaluated_steps()).then_some(level);
                    tab.grid.set_preview(preview, doc);
                }
            }
            Message::RemoveStep(_) | Message::MoveStep(..) | Message::EditStep(_)
                if self.active.is_some_and(|id| !self.steps_changeable(id)) =>
            {
                if let Some(id) = self.active {
                    self.not_editable(id);
                }
            }
            Message::RemoveStep(index) => {
                if let Some(id) = self.active {
                    self.change_steps(id, index, Command::RemoveStep { index });
                }
            }
            Message::MoveStep(from, to) => {
                if let Some(id) = self.active {
                    self.change_steps(id, from.min(to), Command::MoveStep { from, to });
                }
            }
            Message::EditStep(index) => {
                if let Some(id) = self.active {
                    return self.edit_step(id, index);
                }
            }
            Message::EditLeaf(pointer, data_type, text) => {
                if let Some(id) = self.active {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    let selected = self
                        .tabs
                        .iter()
                        .find(|t| t.id == id)
                        .and_then(|t| t.grid.selected());
                    if let Some((row, column)) = selected {
                        self.edit = None;
                        self.leaf_edit = Some(LeafEdit {
                            document: id,
                            row,
                            column,
                            pointer,
                            data_type,
                            text,
                            error: None,
                            rename: false,
                        });
                        return iced::widget::operation::focus(LEAF_ID);
                    }
                }
            }
            Message::EditKey(pointer, key) => {
                let selected = self.active.filter(|&id| self.editable(id)).and_then(|id| {
                    let tab = self.tabs.iter().find(|t| t.id == id)?;
                    Some((id, tab.grid.selected()?))
                });
                if let Some((id, (row, column))) = selected {
                    self.edit = None;
                    self.leaf_edit = Some(LeafEdit {
                        document: id,
                        row,
                        column,
                        pointer,
                        data_type: veta_core::arrow::datatypes::DataType::Utf8,
                        text: key,
                        error: None,
                        rename: true,
                    });
                    return iced::widget::operation::focus(LEAF_ID);
                }
            }
            Message::Pasted(text) => {
                if let (Some(id), Some(text)) = (self.active, text) {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    self.paste(id, &text);
                }
            }
            Message::OpenJsonEditor => {
                if let Some(id) = self.active {
                    self.open_json_editor(id);
                }
            }
            Message::Json(message) => {
                if let Some(Dialog::Json { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, dialogs::JsonMessage::Cancel);
                    if let Some(value) = dialog.update(message) {
                        let command = Command::SetCell {
                            row: dialog.row,
                            column: dialog.column.clone(),
                            value,
                        };
                        let result = self
                            .workbook
                            .get_mut(id)
                            .map(|doc| controller::execute(doc, command));
                        match result {
                            Some(Ok(())) => {
                                self.dialog = None;
                                self.refresh(id);
                            }
                            Some(Err(e)) => dialog.set_error(e.to_string()),
                            None => self.dialog = None,
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::NestedEdit(pointer, change) => {
                let Some(id) = self.active else {
                    return Task::none();
                };
                if !self.editable(id) {
                    self.not_editable(id);
                    return Task::none();
                }
                let Some((row, column)) = self
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.grid.selected())
                else {
                    return Task::none();
                };
                self.leaf_edit = None;
                let result = self.change_nested_cell(id, row, column, |json| {
                    veta_core::values::edit_nested(json, &pointer, &change)
                });
                match result {
                    Ok(()) => self.refresh(id),
                    Err(e) => self.errors.push(e),
                }
            }
            Message::LeafInput(text) => {
                if let Some(edit) = &mut self.leaf_edit {
                    edit.text = text;
                    edit.error = None;
                }
            }
            Message::CommitLeaf => self.commit_leaf(),
            Message::ToggleNode(path) => {
                if !self.collapsed.remove(&path) {
                    self.collapsed.insert(path);
                }
            }
            Message::StatsProgress(id, revision, progress) => {
                if let Some(run) = &mut self.stats.running
                    && run.document == id
                    && run.revision == revision
                {
                    run.progress = progress;
                }
            }
            Message::StatsDone(id, revision, result) => {
                if self
                    .stats
                    .running
                    .as_ref()
                    .is_some_and(|r| r.document == id && r.revision == revision)
                {
                    self.stats.running = None;
                }
                let current = self.workbook.get(id).map(Document::revision);
                // Keep only results for the document's current data.
                self.stats
                    .cache
                    .retain(|(d, r, _), _| *d != id || Some(*r) == current);
                match result {
                    Ok(stats) if current == Some(revision) => {
                        for s in stats {
                            self.stats.cache.insert((id, revision, s.column.clone()), s);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => self
                        .errors
                        .push(format!("Could not compute statistics: {e}")),
                }
            }
            Message::ImportPicked(Some(path)) => return self.open(vec![path]),
            Message::ImportPicked(None) => {}
            Message::Import(message) => {
                if let Some(Dialog::Import(dialog)) = &mut self.dialog {
                    let cancel = matches!(message, ImportMessage::Cancel);
                    if let Some(options) = dialog.update(message) {
                        let path = dialog.path.clone();
                        self.dialog = None;
                        let budget = self.config.memory_budget();
                        self.opening.push(path.clone());
                        let worker_path = path.clone();
                        return self.start_transfer(
                            Some(path.clone()),
                            format!("Importing {}", file_label(&path)),
                            move |progress| {
                                veta_core::convert::import_csv(
                                    &worker_path,
                                    &options,
                                    budget,
                                    progress,
                                )
                                .map(Slot::new)
                                .map_err(|e| e.to_string())
                            },
                            move |result| Message::Imported(path.clone(), result),
                        );
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Export(message) => {
                if let Some(Dialog::Export { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, ExportMessage::Cancel);
                    let submitted = dialog.update(message).is_some();
                    let kind = export_kind(dialog.choice);
                    if submitted {
                        let doc = self.workbook.get(id);
                        let name = doc
                            .map(|d| files::suggested_name(&d.title(), kind))
                            .unwrap_or_default();
                        let dir = doc
                            .and_then(Document::path)
                            .and_then(Path::parent)
                            .map(Path::to_path_buf);
                        let save = files::save("Export".into(), kind, name, dir);
                        return Task::perform(save, move |p| Message::ExportTo(id, p));
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::ExportTo(id, Some(path)) => {
                let Some(Dialog::Export { dialog, .. }) = self.dialog.take() else {
                    return Task::none();
                };
                let format = dialog.format();
                let output = match self.workbook.get(id).map(Document::output) {
                    Some(Ok(output)) => output,
                    Some(Err(e)) => {
                        self.errors.push(format!("Could not export: {e}"));
                        return Task::none();
                    }
                    None => return Task::none(),
                };
                return self.start_transfer(
                    None,
                    format!("Exporting {}", file_label(&path)),
                    move |progress| {
                        veta_core::convert::export(&output, &path, &format, progress)
                            .map(|()| path)
                            .map_err(|e| e.to_string())
                    },
                    Message::Exported,
                );
            }
            Message::ExportTo(_, None) => {}
            Message::Exported(result) => match result {
                Ok(path) => self.notice = Some(format!("Exported {}", path.display())),
                Err(e) if is_cancelled(&e) => self.notice = Some("Export cancelled".into()),
                Err(e) => self.errors.push(format!("Could not export: {e}")),
            },
            Message::TransferProgress(id, progress) => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                    t.progress = progress;
                }
            }
            Message::TransferDone(id) => self.transfers.retain(|t| t.id != id),
            Message::CancelTransfer(id) => {
                if let Some(t) = self.transfers.iter().find(|t| t.id == id) {
                    t.cancel.store(true, Ordering::Relaxed);
                }
            }
            Message::ClearFilter => {
                if let Some(id) = self.active {
                    self.clear_filter(id);
                }
            }
            Message::SaveRecipe => {
                if let Some(id) = self.active {
                    let name = self
                        .workbook
                        .get(id)
                        .map(|d| {
                            let title = d.title();
                            let stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
                            format!("{stem}-recipe.json")
                        })
                        .unwrap_or_default();
                    let save = files::save("Save recipe".into(), FileKind::Recipe, name, None);
                    return Task::perform(save, move |path| Message::SaveRecipeTo(id, path));
                }
            }
            Message::SaveRecipeTo(id, Some(path)) => {
                if let Some(doc) = self.workbook.get(id) {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.trim_end_matches(".json").trim_end_matches(".recipe"))
                        .unwrap_or_default()
                        .to_owned();
                    let info = veta_core::recipe::RecipeInfo {
                        name,
                        description: String::new(),
                        source_file: doc
                            .path()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned()),
                    };
                    let result = veta_core::recipe::to_json(doc.steps(), &info)
                        .map_err(|e| e.to_string())
                        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
                    if let Err(e) = result {
                        self.errors.push(format!("Could not save the recipe: {e}"));
                    }
                }
            }
            Message::LoadRecipe => {
                if let Some(id) = self.active {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    let open = files::open_one("Load recipe".into(), vec![FileKind::Recipe]);
                    return Task::perform(open, move |path| Message::LoadRecipeFrom(id, path));
                }
            }
            Message::LoadRecipeFrom(id, Some(path)) => {
                let steps = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|text| {
                        let schema = self.workbook.get(id).map(Document::schema);
                        let schema = schema.ok_or_else(|| "the document is closed".to_owned())?;
                        veta_core::recipe::from_json(&text, &schema).map_err(|e| e.to_string())
                    });
                match steps {
                    Ok(steps) if steps.is_empty() => {
                        self.errors.push("The recipe has no steps.".into());
                    }
                    Ok(steps) => {
                        if self.execute(id, Command::AppendSteps { steps }) {
                            self.after_compute(id);
                        }
                    }
                    Err(e) => self.errors.push(format!("Could not load the recipe: {e}")),
                }
            }
            Message::SaveRecipeTo(_, None) | Message::LoadRecipeFrom(_, None) => {}
            Message::ConfirmStep(yes) => {
                if let Some(Dialog::ConfirmStep { document, command }) = self.dialog.take()
                    && yes
                {
                    self.execute(document, command);
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
                    if let Some(commands) = dialog.update(message) {
                        let result = match self.workbook.get_mut(id) {
                            Some(doc) => commands
                                .into_iter()
                                .try_for_each(|command| controller::execute(doc, command)),
                            None => Ok(()),
                        };
                        match result {
                            Ok(()) => self.dialog = None,
                            Err(e) => dialog.set_error(e.to_string()),
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
                // Close the innermost thing that is open.
                if self.leaf_edit.take().is_some() || self.menu.take().is_some() {
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

    fn perform(&mut self, action: Action) -> Task<Message> {
        let changes_data = matches!(
            action,
            Action::InsertRows
                | Action::DeleteRows
                | Action::AddColumn
                | Action::RenameColumn
                | Action::RemoveColumns
                | Action::KeepColumns
                | Action::SortAscending
                | Action::SortDescending
                | Action::CustomSort
                | Action::ExpandStruct
                | Action::ExplodeList
                | Action::DataType
                | Action::ReplaceValues
                | Action::FillDown
                | Action::FormatText
                | Action::SplitColumn
                | Action::MergeColumns
                | Action::CombineStruct
                | Action::CustomColumn
                | Action::DuplicateColumn
                | Action::IndexColumn
                | Action::Standard
                | Action::DateTime
                | Action::Cut
                | Action::Paste
                | Action::FillUp
                | Action::MoveColumnLeft
                | Action::MoveColumnRight
                | Action::ExpandAll
                | Action::EditJson
        );
        if changes_data
            && let Some(id) = self.active
            && !self.editable(id)
        {
            self.not_editable(id);
            return Task::none();
        }
        match action {
            Action::Import => {
                let open = files::open_one(
                    "Import".into(),
                    vec![FileKind::Csv, FileKind::Json, FileKind::JsonLines],
                );
                return Task::perform(open, Message::ImportPicked);
            }
            Action::Export => {
                if let Some(id) = self.active {
                    match self.workbook.get(id).map(Document::output) {
                        Some(Ok(_)) => {
                            self.edit = None;
                            self.dialog = Some(Dialog::Export {
                                document: id,
                                dialog: ExportDialog::default(),
                            });
                        }
                        Some(Err(e)) => self.errors.push(format!("Could not export: {e}")),
                        None => {}
                    }
                }
            }
            Action::New => self.new_document(),
            Action::LoadRecipe => return self.handle(Message::LoadRecipe),
            Action::SaveRecipe => return self.handle(Message::SaveRecipe),
            Action::Copy | Action::Cut => {
                if let Some(id) = self.active {
                    return self.copy_selection(id, action == Action::Cut);
                }
            }
            Action::Paste => {
                if self.active.is_some() {
                    return iced::clipboard::read().map(Message::Pasted);
                }
            }
            Action::FillUp => {
                if let Some(id) = self.active {
                    self.fill_selected(id, true);
                }
            }
            Action::MoveColumnLeft | Action::MoveColumnRight => {
                if let Some(id) = self.active
                    && let Some(c) = self.selected_columns(id)
                {
                    let to = if action == Action::MoveColumnLeft {
                        c.start.checked_sub(1)
                    } else {
                        Some(c.start + 1)
                    };
                    if let Some(to) = to {
                        self.move_column(id, c.start, to);
                    }
                }
            }
            Action::ExpandAll => {
                if let Some(id) = self.active {
                    self.nested_step(id, false, true);
                }
            }
            Action::EditJson => {
                if let Some(id) = self.active {
                    self.open_json_editor(id);
                }
            }
            Action::Open => {
                let open = files::open_many("Open".into(), vec![FileKind::Parquet]);
                return Task::perform(open, |files| {
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
                if let Some(id) = self.active
                    && self
                        .workbook
                        .get(id)
                        .is_some_and(|d| matches!(d.status(), Status::Computing { .. }))
                {
                    // Cancel stops the pass (and undoes the change).
                    self.not_editable(id);
                    return Task::none();
                }
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
            Action::DeleteRows => {
                if let Some(id) = self.active {
                    self.delete_rows(id);
                }
            }
            Action::FilterRows => {
                if let Some(id) = self.active {
                    return self.open_filter_dialog(id);
                }
            }
            Action::KeepRows | Action::RemoveRows => {
                if let Some(id) = self.active {
                    self.apply_filter(id, action == Action::RemoveRows);
                }
            }
            Action::ReplaceValues => {
                if let Some(id) = self.active {
                    return self.open_replace_dialog(id, None);
                }
            }
            Action::FillDown => {
                if let Some(id) = self.active {
                    self.fill_selected(id, false);
                }
            }
            Action::FormatText => {
                if let Some(id) = self.active {
                    self.open_format_dialog(id);
                }
            }
            Action::CustomColumn | Action::Standard | Action::DateTime => {
                if let Some(id) = self.active {
                    let column = self.selected_column_names(id).into_iter().next();
                    let reference = column.as_deref().map(formula_reference);
                    let expression = match (action, reference) {
                        (Action::Standard, Some(c)) => format!("{c} * 1"),
                        (Action::DateTime, Some(c)) => format!("year({c})"),
                        _ => String::new(),
                    };
                    return self.open_custom_dialog(id, None, expression);
                }
            }
            Action::DuplicateColumn => {
                if let Some(id) = self.active {
                    self.duplicate_column(id);
                }
            }
            Action::IndexColumn => {
                if let Some(id) = self.active {
                    let at = self.column_count(id);
                    let name = self.unique_name(id, "Index");
                    self.execute(
                        id,
                        Command::CustomColumn(CustomColumn {
                            name,
                            expression: "row()".into(),
                            at,
                        }),
                    );
                }
            }
            Action::SplitColumn => {
                if let Some(id) = self.active {
                    return self.open_split_dialog(id, None);
                }
            }
            Action::MergeColumns => {
                if let Some(id) = self.active {
                    return self.open_merge_dialog(id, None);
                }
            }
            Action::CombineStruct => {
                if let Some(id) = self.active {
                    return self.open_combine_dialog(id, None);
                }
            }
            Action::DataType => {
                if let Some(id) = self.active {
                    self.open_cast_dialog(id, None);
                }
            }
            Action::ExpandStruct | Action::ExplodeList => {
                if let Some(id) = self.active {
                    self.nested_step(id, action == Action::ExplodeList, false);
                }
            }
            Action::CustomSort => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    let columns: Vec<String> = doc
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| f.name().clone())
                        .collect();
                    let column = self
                        .selected_column_names(id)
                        .into_iter()
                        .next()
                        .or_else(|| columns.first().cloned())
                        .unwrap_or_default();
                    let sort = Sort {
                        keys: vec![SortKey {
                            column,
                            descending: false,
                            nulls_first: false,
                        }],
                    };
                    self.edit = None;
                    self.dialog = Some(Dialog::Sort {
                        document: id,
                        dialog: SortDialog::new(columns, &sort),
                        replace: None,
                    });
                }
            }
            Action::SortAscending | Action::SortDescending => {
                if let Some(id) = self.active {
                    self.sort_selected(id, action == Action::SortDescending);
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
            Action::RemoveColumns | Action::KeepColumns => {
                if let Some(id) = self.active {
                    self.keep_or_remove_columns(id, action == Action::KeepColumns, false);
                }
            }
            Action::FileMetadata => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    self.dialog = Some(Dialog::Metadata {
                        document: id,
                        dialog: MetadataDialog::new(
                            &doc.metadata().key_value,
                            column_metadata(doc),
                        ),
                    });
                }
            }
            Action::WriterSettings => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    let schema = doc.schema();
                    let columns = veta_core::io::leaf_columns(&schema).unwrap_or_else(|_| {
                        schema
                            .fields()
                            .iter()
                            .map(|f| (f.name().clone(), f.data_type().clone()))
                            .collect()
                    });
                    self.dialog = Some(Dialog::Writer {
                        document: id,
                        dialog: WriterDialog::new(doc.writer_settings().clone(), columns),
                    });
                }
            }
            Action::ToggleDetails => self.show_side_pane = !self.show_side_pane,
            Action::AppliedSteps => {
                self.show_side_pane = true;
                self.side_tab = SideTab::Steps;
            }
            Action::StatisticsPane | Action::Statistics => {
                self.show_side_pane = true;
                self.side_tab = SideTab::Stats;
            }
            Action::Mode(mode) => {
                self.config.appearance.mode = mode;
                self.save_config();
            }
            Action::Settings => self.dialog = Some(Dialog::Settings(Draft::new(&self.config))),
            Action::About => self.dialog = Some(Dialog::About),
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

    /// Opens the settings dialog of the step at `index`.
    fn edit_step(&mut self, id: DocumentId, index: usize) -> Task<Message> {
        let Some(step) = self
            .workbook
            .get(id)
            .and_then(|d| d.steps().get(index))
            .cloned()
        else {
            return Task::none();
        };
        let dialog = match &step {
            Step::RenameColumn { from, to } => ColumnDialog::rename(from.clone()).with_name(to),
            Step::AddColumn {
                name,
                data_type,
                at,
            } => ColumnDialog::add(*at).with_name(name).with_type(data_type),
            Step::Cast(cast) => {
                self.open_cast_dialog(id, Some((index, cast.clone())));
                return Task::none();
            }
            Step::Replace(r) => return self.open_replace_dialog(id, Some((index, r.clone()))),
            Step::Split(s) => return self.open_split_dialog(id, Some((index, s.clone()))),
            Step::CustomColumn(c) => {
                return self.open_custom_dialog(id, Some((index, c.clone())), String::new());
            }
            Step::Merge(m) => return self.open_merge_dialog(id, Some((index, m.clone()))),
            Step::CombineStruct { columns, name } => {
                let step = (columns.clone(), name.clone());
                return self.open_combine_dialog(id, Some((index, step)));
            }
            Step::FormatText { columns, op } => {
                self.edit = None;
                self.dialog = Some(Dialog::Format {
                    document: id,
                    dialog: FormatDialog::new(columns.clone(), *op),
                    replace: Some(index),
                });
                return Task::none();
            }
            Step::Filter(_) | Step::Sort(_) => {
                let schema = self.workbook.get(id).and_then(|d| d.schema_at(index));
                let columns = schema
                    .as_ref()
                    .map(|schema| schema.fields().iter().map(|f| f.name().clone()).collect())
                    .unwrap_or_default();
                let nested = schema.as_deref().map(nested_columns).unwrap_or_default();
                self.edit = None;
                self.dialog = Some(match &step {
                    Step::Filter(filter) => Dialog::Filter {
                        document: id,
                        dialog: FilterDialog::edit(columns, filter).with_nested(nested),
                        replace: Some(index),
                    },
                    Step::Sort(sort) => Dialog::Sort {
                        document: id,
                        dialog: SortDialog::new(columns, sort),
                        replace: Some(index),
                    },
                    _ => return Task::none(),
                });
                return Task::none();
            }
            _ => return Task::none(),
        };
        let task = self.open_column_dialog(id, dialog);
        if let Some(Dialog::Column { replace, .. }) = &mut self.dialog {
            *replace = Some(index);
        }
        task
    }

    /// Runs a step-list command, first asking if later steps refer to rows
    /// by position (they may then apply to different rows).
    fn change_steps(&mut self, id: DocumentId, from: usize, command: Command) {
        let positional_after = self
            .workbook
            .get(id)
            .is_some_and(|d| d.steps().iter().skip(from + 1).any(Step::is_positional));
        if positional_after {
            self.dialog = Some(Dialog::ConfirmStep {
                document: id,
                command,
            });
        } else {
            self.execute(id, command);
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
            self.append_row(id);
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

    /// Adds one row at the end and selects it.
    fn append_row(&mut self, id: DocumentId) {
        self.commit_edit();
        let Some(at) = self.workbook.get(id).map(Document::num_rows) else {
            return;
        };
        if self.execute(id, Command::InsertRows { at, count: 1 })
            && let (Some(doc), Some(tab)) = (
                self.workbook.get(id),
                self.tabs.iter_mut().find(|t| t.id == id),
            )
        {
            tab.grid.select_rows(at..at + 1, doc);
        }
    }

    /// Opens a blank document named `Untitled`, `Untitled 2`, … in a new tab.
    fn new_document(&mut self) {
        let titles: Vec<String> = self.workbook.iter().map(|(_, d)| d.title()).collect();
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    "Untitled".to_owned()
                } else {
                    format!("Untitled {n}")
                }
            })
            .find(|name| !titles.contains(name))
            .unwrap_or_default();
        let mut document = Document::untitled(name);
        document.set_initial_writer_settings(self.config.new_files.settings());
        self.add_tab(document);
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
        // Imported documents are saved as Parquet.
        let name = files::suggested_name(&doc.title(), FileKind::Parquet);
        let dir = doc.path().and_then(Path::parent).map(Path::to_path_buf);
        let save = files::save("Save as".into(), FileKind::Parquet, name, dir);
        Task::perform(save, move |path| Message::SaveTo(id, path))
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

    /// Opens the filter dialog on the selected column.
    fn open_filter_dialog(&mut self, id: DocumentId) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let nested = nested_columns(&doc.schema());
        let columns: Vec<String> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        let selected = self
            .selected_columns(id)
            .and_then(|c| columns.get(c.start).cloned());
        let current = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.grid.filter().cloned());
        let dialog = match current {
            Some(filter) => FilterDialog::edit(columns, &filter),
            None => FilterDialog::new(columns, selected),
        }
        .with_nested(nested);
        self.edit = None;
        self.dialog = Some(Dialog::Filter {
            document: id,
            dialog,
            replace: None,
        });
        iced::widget::operation::focus(dialogs::FILTER_VALUE_ID)
    }

    /// Filters the rows shown (not a step). The filter is computed in the
    /// background; the table is read-only until it is cleared or applied.
    fn set_view_filter(&mut self, id: DocumentId, filter: Filter) -> Result<(), String> {
        let Some(doc) = self.workbook.get(id) else {
            return Ok(());
        };
        if doc.status() != &Status::Ready {
            return Err("Wait until the steps are computed.".into());
        }
        let view = doc.filter_view(filter.clone()).map_err(|e| e.to_string())?;
        self.commit_edit_or_drop();
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.set_filter(Some((filter, view)), doc);
        }
        Ok(())
    }

    fn clear_filter(&mut self, id: DocumentId) {
        if let (Some(doc), Some(tab)) = (
            self.workbook.get(id),
            self.tabs.iter_mut().find(|t| t.id == id),
        ) {
            tab.grid.set_filter(None, doc);
        }
    }

    /// Turns the temporary filter into a step that keeps (or removes) the
    /// rows it shows, and clears it.
    fn apply_filter(&mut self, id: DocumentId, remove: bool) {
        let Some(filter) = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.grid.filter().cloned())
        else {
            return;
        };
        self.clear_filter(id);
        self.execute(
            id,
            Command::Filter(Filter {
                exclude: remove,
                ..filter
            }),
        );
    }

    /// Keeps only the selected rows.
    fn keep_rows(&mut self, id: DocumentId) {
        let (Some(rows), Some(total)) = (
            self.selected_rows(id),
            self.workbook.get(id).map(Document::num_rows),
        ) else {
            return;
        };
        let delete: Vec<_> = [0..rows.start, rows.end..total]
            .into_iter()
            .filter(|r| !r.is_empty())
            .collect();
        if !delete.is_empty() {
            self.edit = None;
            self.execute(id, Command::DeleteRows { rows: delete });
        }
    }

    /// Closes the dialog and adds `command`'s step, or replaces step
    /// `replace` with `step`. An error adding it is returned to show in the
    /// dialog, which then stays open.
    fn add_or_replace(
        &mut self,
        id: DocumentId,
        replace: Option<usize>,
        step: Step,
        command: Command,
    ) -> Result<(), String> {
        if let Some(index) = replace {
            self.dialog = None;
            self.change_steps(id, index, Command::ReplaceStep { index, step });
            return Ok(());
        }
        let Some(doc) = self.workbook.get_mut(id) else {
            return Ok(());
        };
        controller::execute(doc, command).map_err(|e| e.to_string())?;
        self.dialog = None;
        self.refresh(id);
        Ok(())
    }

    /// Opens the custom column dialog (with `expression`), or the custom
    /// column step at `index`.
    fn open_custom_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, CustomColumn)>,
        expression: String,
    ) -> Task<Message> {
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        let level = step.as_ref().map_or(doc.evaluated_steps(), |(i, _)| *i);
        let Some(schema) = doc.schema_at(level) else {
            return Task::none();
        };
        let custom = match &step {
            Some((_, c)) => c.clone(),
            None => CustomColumn {
                name: self.unique_name(id, "Custom"),
                expression,
                at: self
                    .selected_columns(id)
                    .map_or(schema.fields().len(), |c| c.end),
            },
        };
        self.edit = None;
        self.dialog = Some(Dialog::Custom {
            document: id,
            dialog: CustomDialog::new(schema, custom),
            replace: step.map(|(i, _)| i),
        });
        Task::batch([
            iced::widget::operation::focus(dialogs::EXPRESSION_ID),
            iced::widget::operation::move_cursor_to_end(dialogs::EXPRESSION_ID),
        ])
    }

    /// Applies the Value tab edit: the cell gets its JSON with the edited
    /// value replaced.
    fn commit_leaf(&mut self) {
        let Some(mut edit) = self.leaf_edit.take() else {
            return;
        };
        let id = edit.document;
        let text = (!edit.text.is_empty()).then_some(edit.text.clone());
        let (pointer, data_type, rename) =
            (edit.pointer.clone(), edit.data_type.clone(), edit.rename);
        let result = self.change_nested_cell(id, edit.row, edit.column, |json| {
            if rename {
                let key = veta_core::values::NestedEdit::RenameKey(text.unwrap_or_default());
                veta_core::values::edit_nested(json, &pointer, &key)
            } else {
                veta_core::values::set_nested(json, &pointer, text.as_deref(), &data_type)
            }
        });
        match result {
            Ok(()) => self.refresh(id),
            Err(e) => {
                edit.error = Some(e);
                self.leaf_edit = Some(edit);
            }
        }
    }

    /// Copies the selection to the clipboard as tab-separated text (whole
    /// columns with their names first). With `cut`, then clears the cell or
    /// removes the rows or columns.
    fn copy_selection(&mut self, id: DocumentId, cut: bool) -> Task<Message> {
        self.commit_edit();
        let (Some(tab), Some(doc)) = (self.tabs.iter().find(|t| t.id == id), self.workbook.get(id))
        else {
            return Task::none();
        };
        let selection = tab.grid.selection();
        let text = match &selection {
            Selection::None => return Task::none(),
            Selection::Cell(r, c) => {
                veta_core::clipboard::copy_text(doc, *r..r + 1, *c..c + 1, false)
            }
            Selection::Rows(rows) => {
                veta_core::clipboard::copy_text(doc, rows.clone(), 0..doc.num_columns(), false)
            }
            Selection::Columns(cols) => {
                veta_core::clipboard::copy_text(doc, 0..doc.num_rows(), cols.clone(), true)
            }
        };
        let text = match text {
            Ok(text) => text,
            Err(e) => {
                self.errors.push(format!("Could not copy: {e}"));
                return Task::none();
            }
        };
        if cut {
            match selection {
                Selection::Cell(row, c) => {
                    let column = doc.schema().field(c).name().clone();
                    self.execute(
                        id,
                        Command::SetCell {
                            row,
                            column,
                            value: None,
                        },
                    );
                }
                Selection::Rows(_) => self.delete_rows(id),
                Selection::Columns(_) => self.keep_or_remove_columns(id, false, true),
                Selection::None => {}
            }
        }
        iced::clipboard::write(text)
    }

    /// Pastes tab-separated `text` at the selected cell (or the start of
    /// the selected rows or columns) as one change.
    fn paste(&mut self, id: DocumentId, text: &str) {
        self.commit_edit();
        let (Some(tab), Some(doc)) = (self.tabs.iter().find(|t| t.id == id), self.workbook.get(id))
        else {
            return;
        };
        let mut values = veta_core::clipboard::parse_tsv(text);
        let (row, column) = match tab.grid.selection() {
            Selection::None => {
                self.errors.push("Select where to paste first.".into());
                return;
            }
            Selection::Cell(r, c) => (r, c),
            Selection::Rows(rows) => (rows.start, 0),
            Selection::Columns(cols) => {
                // Copied whole columns start with their names.
                let names = self.column_names(id, cols.clone());
                let header: Vec<Option<String>> = names.into_iter().map(Some).collect();
                if values.first() == Some(&header) {
                    values.remove(0);
                }
                (0, cols.start)
            }
        };
        if values.is_empty() || doc.num_columns() == 0 {
            return;
        }
        self.execute(
            id,
            Command::PasteCells {
                row,
                column,
                values,
            },
        );
    }

    /// Opens the selected nested cell in the multi-line JSON editor.
    fn open_json_editor(&mut self, id: DocumentId) {
        if !self.editable(id) {
            self.not_editable(id);
            return;
        }
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return;
        };
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let Some((row, column)) = tab.grid.selected() else {
            return;
        };
        let schema = doc.schema();
        let field = schema.field(column);
        if !field.data_type().is_nested() {
            return;
        }
        let Some(json) = tab
            .grid
            .cell_array(row, column, doc)
            .and_then(|a| veta_core::display::value_json(a.as_ref(), 0).ok())
        else {
            return;
        };
        self.edit = None;
        self.leaf_edit = None;
        self.dialog = Some(Dialog::Json {
            document: id,
            dialog: dialogs::JsonDialog::new(row, field.name().clone(), json.as_deref()),
        });
    }

    /// Replaces a nested cell with `change` applied to its JSON (`null` for
    /// a null cell).
    fn change_nested_cell(
        &mut self,
        id: DocumentId,
        row: usize,
        column: usize,
        change: impl FnOnce(&str) -> veta_core::Result<String>,
    ) -> Result<(), String> {
        let tab = self.tabs.iter().find(|t| t.id == id).ok_or("closed")?;
        let doc = self.workbook.get(id).ok_or("closed")?;
        let json = tab
            .grid
            .cell_array(row, column, doc)
            .and_then(|a| veta_core::display::value_json(a.as_ref(), 0).ok())
            .ok_or("the cell isn't loaded")?
            .unwrap_or_else(|| "null".into());
        let value = change(&json).map_err(|e| e.to_string())?;
        let name = doc.schema().field(column).name().clone();
        let doc = self.workbook.get_mut(id).ok_or("closed")?;
        controller::execute(
            doc,
            Command::SetCell {
                row,
                column: name,
                value: Some(value),
            },
        )
        .map_err(|e| e.to_string())
    }

    /// Expands the selected struct column, or explodes the selected list
    /// column into rows.
    fn nested_step(&mut self, id: DocumentId, explode: bool, all_levels: bool) {
        let Some(column) = self.selected_column_names(id).into_iter().next() else {
            self.errors.push("Select a column first.".into());
            return;
        };
        let command = if explode {
            Command::ExplodeList { column }
        } else {
            Command::ExpandStruct { column, all_levels }
        };
        self.execute(id, command);
    }

    /// Adds a copy of the selected column next to it.
    fn duplicate_column(&mut self, id: DocumentId) {
        let Some(range) = self.selected_columns(id).or_else(|| {
            let (_, c) = self.tabs.iter().find(|t| t.id == id)?.grid.selected()?;
            Some(c..c + 1)
        }) else {
            self.errors.push("Select a column to duplicate.".into());
            return;
        };
        let Some(column) = self.column_names(id, range.start..range.start + 1).pop() else {
            return;
        };
        let name = self.unique_name(id, &format!("{column} - Copy"));
        self.execute(
            id,
            Command::CustomColumn(CustomColumn {
                name,
                expression: formula_reference(&column),
                at: range.start + 1,
            }),
        );
    }

    /// `base`, or `base 2`, `base 3`… if a column already has that name.
    fn unique_name(&self, id: DocumentId, base: &str) -> String {
        let Some(doc) = self.workbook.get(id) else {
            return base.to_owned();
        };
        let schema = doc.schema();
        let taken = |name: &str| schema.fields().iter().any(|f| f.name() == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Opens the split dialog on the selected column, or on the split step
    /// at `index`.
    fn open_split_dialog(&mut self, id: DocumentId, step: Option<(usize, Split)>) -> Task<Message> {
        let column = match &step {
            Some((_, s)) => Some(s.column.clone()),
            None => self.selected_column_names(id).into_iter().next(),
        };
        let Some(column) = column else {
            self.errors.push("Select a column to split.".into());
            return Task::none();
        };
        let Some(sample) = self.sample(id, step.as_ref().map(|(i, _)| *i), &column) else {
            return Task::none();
        };
        self.edit = None;
        self.dialog = Some(Dialog::Split {
            document: id,
            dialog: SplitDialog::new(column, sample, step.as_ref().map(|(_, s)| s)),
            replace: step.map(|(i, _)| i),
        });
        iced::widget::operation::focus(dialogs::NAME_ID)
    }

    /// Opens the merge dialog on the selected columns, or on the merge step
    /// at `index`.
    fn open_merge_dialog(&mut self, id: DocumentId, step: Option<(usize, Merge)>) -> Task<Message> {
        let columns = match &step {
            Some((_, m)) => m.columns.clone(),
            None => self.selected_column_names(id),
        };
        if columns.len() < 2 {
            self.errors
                .push("Select two or more columns to merge (Shift+click the headers).".into());
            return Task::none();
        }
        self.edit = None;
        self.dialog = Some(Dialog::Merge {
            document: id,
            dialog: MergeDialog::new(columns, step.as_ref().map(|(_, m)| m)),
            replace: step.map(|(i, _)| i),
        });
        Task::batch([
            iced::widget::operation::focus(dialogs::NAME_ID),
            iced::widget::operation::select_all(dialogs::NAME_ID),
        ])
    }

    /// Opens the combine dialog on the selected columns, or on the combine
    /// step at `index`.
    fn open_combine_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, (Vec<String>, String))>,
    ) -> Task<Message> {
        let (replace, columns, name) = match step {
            Some((i, (columns, name))) => (Some(i), columns, name),
            None => {
                let columns = self.selected_column_names(id);
                // Columns from one expanded struct (`a.x`, `a.y`) go back
                // into `a`.
                let prefix = columns
                    .first()
                    .and_then(|c| c.rsplit_once('.'))
                    .map(|(p, _)| p.to_owned())
                    .filter(|p| {
                        columns.iter().all(|c| {
                            c.strip_prefix(p.as_str())
                                .is_some_and(|r| r.starts_with('.'))
                        })
                    });
                let name = prefix.unwrap_or_else(|| self.unique_name(id, "Struct"));
                (None, columns, name)
            }
        };
        if columns.is_empty() {
            self.errors
                .push("Select the columns to combine (Shift+click the headers).".into());
            return Task::none();
        }
        self.edit = None;
        self.dialog = Some(Dialog::Combine {
            document: id,
            dialog: CombineDialog::new(columns, name),
            replace,
        });
        Task::batch([
            iced::widget::operation::focus(dialogs::NAME_ID),
            iced::widget::operation::select_all(dialogs::NAME_ID),
        ])
    }

    /// The first rows of `column` after `level` steps (all evaluated steps
    /// when `None`).
    fn sample(&self, id: DocumentId, level: Option<usize>, column: &str) -> Option<ArrayRef> {
        let doc = self.workbook.get(id)?;
        let level = level.unwrap_or(doc.evaluated_steps());
        let batch = doc.read_at(level, 0..SAMPLE_ROWS).ok()?;
        batch.column_by_name(column).cloned()
    }

    /// Names of the selected columns, or of the selected cell's column.
    fn selected_column_names(&self, id: DocumentId) -> Vec<String> {
        let range = self.selected_columns(id).or_else(|| {
            let (_, c) = self.tabs.iter().find(|t| t.id == id)?.grid.selected()?;
            Some(c..c + 1)
        });
        range.map_or_else(Vec::new, |r| self.column_names(id, r))
    }

    /// Opens the replace values dialog on the selected column, or on the
    /// replace step at `index`.
    fn open_replace_dialog(
        &mut self,
        id: DocumentId,
        step: Option<(usize, Replace)>,
    ) -> Task<Message> {
        let column = match &step {
            Some((_, r)) => Some(r.column.clone()),
            None => self.selected_column_names(id).into_iter().next(),
        };
        let Some(column) = column else {
            self.errors
                .push("Select a column to replace values in.".into());
            return Task::none();
        };
        let level = step.as_ref().map(|(i, _)| *i);
        let text_column = self
            .workbook
            .get(id)
            .and_then(|d| match level {
                Some(i) => d.schema_at(i),
                None => Some(d.schema()),
            })
            .and_then(|s| {
                let field = s.field_with_name(&column).ok()?;
                Some(veta_core::values::is_text(field.data_type()))
            })
            .unwrap_or(true);
        self.edit = None;
        self.dialog = Some(Dialog::Replace {
            document: id,
            dialog: ReplaceDialog::new(column, text_column, step.as_ref().map(|(_, r)| r)),
            replace: level,
        });
        iced::widget::operation::focus(dialogs::NAME_ID)
    }

    /// Opens the format text dialog on the selected columns.
    fn open_format_dialog(&mut self, id: DocumentId) {
        let columns = self.selected_column_names(id);
        if columns.is_empty() {
            self.errors
                .push("Select the text columns to format.".into());
            return;
        }
        self.edit = None;
        self.dialog = Some(Dialog::Format {
            document: id,
            dialog: FormatDialog::new(columns, TextOp::Trim),
            replace: None,
        });
    }

    /// Fills nulls in the selected columns.
    fn fill_selected(&mut self, id: DocumentId, up: bool) {
        let columns = self.selected_column_names(id);
        if columns.is_empty() {
            self.errors.push("Select the columns to fill.".into());
            return;
        }
        self.execute(id, Command::Fill(Fill { columns, up }));
    }

    /// Opens the change type dialog on the selected column, or on the cast
    /// step at `index` (previewed on that step's input).
    fn open_cast_dialog(&mut self, id: DocumentId, step: Option<(usize, Cast)>) {
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let level = step.as_ref().map_or(doc.evaluated_steps(), |(i, _)| *i);
        let column = match &step {
            Some((_, cast)) => Some(cast.column.clone()),
            None => {
                let index = self.selected_columns(id).map(|c| c.start).or_else(|| {
                    self.tabs
                        .iter()
                        .find(|t| t.id == id)?
                        .grid
                        .selected()
                        .map(|s| s.1)
                });
                index.and_then(|c| self.column_names(id, c..c + 1).pop())
            }
        };
        let Some(column) = column else {
            self.errors
                .push("Select a column to change its type.".into());
            return;
        };
        let Some(sample) = self.sample(id, Some(level), &column) else {
            return;
        };
        self.edit = None;
        self.dialog = Some(Dialog::Cast {
            document: id,
            dialog: CastDialog::new(column, sample, step.as_ref().map(|(_, c)| c)),
            replace: step.map(|(i, _)| i),
        });
    }

    /// Sorts by the selected column.
    fn sort_selected(&mut self, id: DocumentId, descending: bool) {
        let column = self.selected_columns(id).map(|c| c.start).or_else(|| {
            self.tabs
                .iter()
                .find(|t| t.id == id)?
                .grid
                .selected()
                .map(|s| s.1)
        });
        let Some(column) = column.and_then(|c| self.column_names(id, c..c + 1).pop()) else {
            self.errors.push("Select a column to sort by.".into());
            return;
        };
        let sort = Sort {
            keys: vec![SortKey {
                column,
                descending,
                nulls_first: false,
            }],
        };
        self.execute(id, Command::Sort(sort));
    }

    /// Keeps (or removes) the rows whose value in the selected cell's
    /// column equals that cell's.
    fn quick_filter(&mut self, id: DocumentId, keep: bool) {
        let Some((_, column)) = self.selected_cell(id) else {
            return;
        };
        let Some(value) = self.cell_value(id) else {
            return;
        };
        let op = match (&value, keep) {
            (Some(_), true) => FilterOp::Equals,
            (Some(_), false) => FilterOp::NotEquals,
            (None, true) => FilterOp::IsNull,
            (None, false) => FilterOp::IsNotNull,
        };
        let condition = Condition {
            column,
            op,
            value: value.unwrap_or_default(),
            case_sensitive: true,
        };
        let current = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.grid.filter().cloned());
        let filter = match current {
            Some(mut f) if f.match_all => {
                f.conditions.push(condition);
                f
            }
            _ => Filter {
                conditions: vec![condition],
                match_all: true,
                exclude: false,
            },
        };
        if let Err(e) = self.set_view_filter(id, filter) {
            self.errors.push(e);
        }
    }

    /// Reacts to a document's pass finishing: a failed error-column check
    /// is dropped (keeping the errors); other failures are reported.
    fn after_compute(&mut self, id: DocumentId) {
        let Some(doc) = self.workbook.get_mut(id) else {
            return;
        };
        let check = self.error_check.filter(|(d, _)| *d == id);
        match doc.status().clone() {
            Status::Broken { step, error } if check == Some((id, step)) => {
                self.error_check = None;
                let column = match doc.steps().get(step) {
                    Some(Step::RemoveEmptyColumn { name }) => name.clone(),
                    _ => return,
                };
                if controller::amend(doc, Command::RemoveStep { index: step }).is_ok() {
                    let count = error
                        .split_once(": ")
                        .map_or(String::new(), |(_, rest)| format!(" ({rest})"));
                    self.errors.push(format!(
                        "Some values couldn't be converted{count}. They're kept in column \
                         {column}."
                    ));
                }
            }
            Status::Broken { step, error } => {
                let label = doc
                    .steps()
                    .get(step)
                    .map(Step::describe)
                    .unwrap_or_default();
                self.errors
                    .push(format!("Stopped at step {} ({label}): {error}", step + 1));
            }
            Status::Ready if check.is_some() => self.error_check = None,
            _ => {}
        }
    }

    /// Columns the statistics pane shows: the selected ones.
    fn stats_columns(&self, id: DocumentId) -> Vec<String> {
        self.selected_column_names(id)
    }

    /// Starts computing statistics the pane needs and doesn't have, and
    /// stops a pass that is no longer needed.
    fn schedule_stats(&mut self) -> Task<Message> {
        let visible = self.show_side_pane && self.side_tab == SideTab::Stats;
        let wanted = self
            .active
            .filter(|_| visible)
            .and_then(|id| {
                let doc = self.workbook.get(id)?;
                (doc.status() == &Status::Ready).then(|| (id, doc.revision()))
            })
            .map(|(id, revision)| {
                let missing: Vec<String> = self
                    .stats_columns(id)
                    .into_iter()
                    .filter(|c| !self.stats.cache.contains_key(&(id, revision, c.clone())))
                    .collect();
                (id, revision, missing)
            })
            .filter(|(_, _, missing)| !missing.is_empty());
        if let Some(run) = &self.stats.running {
            let still_needed = wanted.as_ref().is_some_and(|(id, revision, missing)| {
                run.document == *id
                    && run.revision == *revision
                    && missing.iter().all(|c| run.columns.contains(c))
            });
            if still_needed {
                return Task::none();
            }
            run.cancel.store(true, Ordering::Relaxed);
            self.stats.running = None;
        }
        let Some((id, revision, columns)) = wanted else {
            return Task::none();
        };
        let Some(job) = self
            .workbook
            .get(id)
            .and_then(|d| d.stats_job(columns.clone()).ok())
        else {
            return Task::none();
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.stats.running = Some(StatsRun {
            document: id,
            revision,
            columns,
            progress: 0.0,
            cancel: cancel.clone(),
        });
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut reported = 0.0;
            let result = job.run(&mut |progress| {
                if progress - reported >= 0.01 {
                    reported = progress;
                    let _ = sender.unbounded_send(Message::StatsProgress(id, revision, progress));
                }
                !cancel.load(Ordering::Relaxed)
            });
            if !cancel.load(Ordering::Relaxed) {
                let result = result.map_err(|e| e.to_string());
                let _ = sender.unbounded_send(Message::StatsDone(id, revision, result));
            }
        });
        Task::run(receiver, |message| message)
    }

    /// The selected cell's value as a tree, for the Value tab.
    fn value_tree(&self, tab: &Tab, doc: &Document) -> Option<veta_core::display::ValueNode> {
        if self.side_tab != SideTab::Value || !self.show_side_pane {
            return None;
        }
        let (row, column) = tab.grid.selected()?;
        let array = tab.grid.cell_array(row, column, doc)?;
        let name = doc
            .schema_at(doc.evaluated_steps())?
            .field(column)
            .name()
            .clone();
        Some(veta_core::display::value_tree(array.as_ref(), 0, &name))
    }

    /// What the statistics pane shows for document `id`.
    fn stats_view(&self, id: DocumentId) -> panes::StatsView {
        let Some(doc) = self.workbook.get(id) else {
            return panes::StatsView::default();
        };
        let revision = doc.revision();
        let columns = self
            .stats_columns(id)
            .into_iter()
            .map(|c| {
                let full = self.stats.cache.get(&(id, revision, c.clone())).cloned();
                match full {
                    Some(stats) => (c, Some(stats), false),
                    None => {
                        let quick = doc.quick_stats(&c);
                        (c, quick, true)
                    }
                }
            })
            .collect();
        let progress = self
            .stats
            .running
            .as_ref()
            .filter(|r| r.document == id && r.revision == revision)
            .map(|r| r.progress);
        let waiting = match doc.status() {
            Status::Ready => None,
            Status::Computing { .. } => Some("Waiting for the steps to be computed…"),
            Status::Broken { .. } => Some("A step has an error; fix it to see statistics."),
        };
        panes::StatsView {
            columns,
            progress,
            waiting,
        }
    }

    /// Starts the next pending full pass if none is running, and stops a
    /// running one whose result would no longer apply.
    fn schedule_compute(&mut self) -> Task<Message> {
        if let Some(c) = &self.computing {
            let current = match c.target {
                Target::Document => self
                    .workbook
                    .get(c.document)
                    .and_then(Document::compute_key),
                Target::View => self
                    .tabs
                    .iter()
                    .find(|t| t.id == c.document)
                    .and_then(|t| t.grid.filter_key()),
            };
            if current.as_ref() != Some(&c.key) {
                c.cancel.store(true, Ordering::Relaxed);
                self.computing = None;
            }
        }
        if self.computing.is_some() {
            return Task::none();
        }
        // The active document first.
        let mut order: Vec<DocumentId> = self.tabs.iter().map(|t| t.id).collect();
        order.sort_by_key(|id| Some(*id) != self.active);
        for id in order {
            if let Some(job) = self.workbook.get(id).and_then(Document::compute_job) {
                return self.start_compute(id, Target::Document, job);
            }
            let view = self.tabs.iter().find(|t| t.id == id);
            if let Some(job) = view.and_then(|t| t.grid.filter_job()) {
                return self.start_compute(id, Target::View, job);
            }
        }
        Task::none()
    }

    fn start_compute(&mut self, id: DocumentId, target: Target, job: ComputeJob) -> Task<Message> {
        let key = job.key().to_vec();
        let cancel = Arc::new(AtomicBool::new(false));
        self.computing = Some(Computing {
            document: id,
            target,
            key: key.clone(),
            step: job.step(),
            progress: 0.0,
            cancel: cancel.clone(),
        });
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut reported = 0.0;
            let result = job.run(&mut |progress| {
                if progress - reported >= 0.01 {
                    reported = progress;
                    let _ = sender.unbounded_send(Message::ComputeProgress(
                        id,
                        target,
                        key.clone(),
                        progress,
                    ));
                }
                !cancel.load(Ordering::Relaxed)
            });
            if !cancel.load(Ordering::Relaxed) {
                let result = result.map(Slot::new).map_err(|e| e.to_string());
                let _ = sender.unbounded_send(Message::ComputeDone(id, target, key, result));
            }
        });
        Task::run(receiver, |message| message)
    }

    /// Runs `work` on a thread with progress and cancel shown in the status
    /// bar; `done` turns its result into the message to handle.
    fn start_transfer<T: Send + 'static>(
        &mut self,
        importing: Option<PathBuf>,
        label: String,
        work: impl FnOnce(&mut dyn FnMut(f32) -> bool) -> T + Send + 'static,
        done: impl Fn(T) -> Message + Send + 'static,
    ) -> Task<Message> {
        let id = self.next_transfer;
        self.next_transfer += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.transfers.push(Transfer {
            id,
            importing,
            label,
            progress: 0.0,
            cancel: cancel.clone(),
        });
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut reported = 0.0;
            let result = work(&mut |progress| {
                if progress - reported >= 0.01 {
                    reported = progress;
                    let _ = sender.unbounded_send(Message::TransferProgress(id, progress));
                }
                !cancel.load(Ordering::Relaxed)
            });
            let _ = sender.unbounded_send(done(result));
            let _ = sender.unbounded_send(Message::TransferDone(id));
        });
        Task::run(receiver, |message| message)
    }

    fn open_column_dialog(&mut self, id: DocumentId, dialog: ColumnDialog) -> Task<Message> {
        self.edit = None;
        self.dialog = Some(Dialog::Column {
            document: id,
            dialog,
            replace: None,
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

    /// Keeps or removes columns. From the ribbon, with at most one column
    /// selected, a dialog asks which; otherwise the selected columns are
    /// used directly.
    fn keep_or_remove_columns(&mut self, id: DocumentId, keep: bool, from_menu: bool) {
        self.edit = None;
        let selected = self
            .selected_columns(id)
            .map(|c| self.column_names(id, c))
            .unwrap_or_default();
        if from_menu || selected.len() > 1 {
            if selected.is_empty() {
                return;
            }
            let command = if keep {
                Command::KeepColumns { names: selected }
            } else {
                Command::RemoveColumns { names: selected }
            };
            self.execute(id, command);
            return;
        }
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let names: Vec<String> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        self.dialog = Some(Dialog::Choose {
            document: id,
            dialog: ChooseColumns::new(keep, names, &selected),
        });
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

    /// The struct field shown in the selected cell, if it is one.
    fn selected_struct_field(&self, id: DocumentId) -> Option<StructField> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (path, data_type) = tab.grid.selected_field()?;
        (!path.is_empty()).then(|| StructField {
            path: path.to_vec(),
            data_type: data_type.clone(),
        })
    }

    /// Sets one field of a struct cell (`None` or empty: null).
    fn set_field(
        &mut self,
        id: DocumentId,
        row: usize,
        column: &str,
        field: &StructField,
        text: Option<&str>,
    ) -> Result<(), String> {
        let index = self
            .workbook
            .get(id)
            .and_then(|d| d.schema().index_of(column).ok())
            .ok_or("the column no longer exists")?;
        let pointer: Vec<_> = field
            .path
            .iter()
            .cloned()
            .map(veta_core::display::PathPart::Field)
            .collect();
        self.change_nested_cell(id, row, index, |json| {
            veta_core::values::set_nested(json, &pointer, text, &field.data_type)
        })
    }

    /// Starts editing the selected cell in the formula bar.
    fn start_edit(&mut self, id: DocumentId, initial: Option<String>) -> Task<Message> {
        let Some((row, column)) = self.selected_cell(id) else {
            return Task::none();
        };
        let text = initial.unwrap_or_else(|| self.cell_text(id).unwrap_or_default());
        let field = self.selected_struct_field(id);
        self.edit = Some(Edit {
            document: id,
            row,
            column,
            field,
            text,
            error: None,
        });
        Task::batch([
            iced::widget::operation::focus(EDITOR_ID),
            iced::widget::operation::move_cursor_to_end(EDITOR_ID),
        ])
    }

    /// Text of the selected cell (empty for null).
    fn cell_text(&self, id: DocumentId) -> Option<String> {
        Some(self.cell_value(id)?.unwrap_or_default())
    }

    /// Text of the selected cell (`None` inside for null).
    fn cell_value(&self, id: DocumentId) -> Option<Option<String>> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let (row, column) = tab.grid.selected()?;
        match tab.grid.selected_edit_value() {
            Some(value) => Some(value.map(str::to_owned)),
            None if self.selected_struct_field(id).is_some() => None,
            None => {
                let batch = self.workbook.get(id)?.read(row..row + 1).ok()?;
                let cells = veta_core::display::format_batch(&batch).ok()?;
                Some(cells.first()?.get(column)?.clone())
            }
        }
    }

    /// Applies the formula bar edit, if any. Returns whether it was applied.
    ///
    /// Invalid input keeps the edit open with the error shown next to it.
    fn commit_edit(&mut self) -> bool {
        let Some(mut edit) = self.edit.take() else {
            return false;
        };
        if self.cell_text(edit.document).as_deref() == Some(edit.text.as_str()) {
            return true;
        }
        let result = if let Some(field) = &edit.field {
            let text = (!edit.text.is_empty()).then_some(edit.text.as_str());
            self.set_field(edit.document, edit.row, &edit.column, field, text)
        } else {
            let command = Command::SetCell {
                row: edit.row,
                column: edit.column.clone(),
                value: Some(edit.text.clone()),
            };
            match self.workbook.get_mut(edit.document) {
                Some(doc) => controller::execute(doc, command).map_err(|e| e.to_string()),
                None => return false,
            }
        };
        match result {
            Ok(()) => {
                self.refresh(edit.document);
                true
            }
            Err(e) => {
                edit.error = Some(e);
                self.edit = Some(edit);
                false
            }
        }
    }

    /// Applies the edit before the selection moves away; if it can't be
    /// applied it is dropped and the error shown as a banner.
    fn commit_edit_or_drop(&mut self) {
        if !self.commit_edit()
            && let Some(edit) = self.edit.take()
            && let Some(error) = edit.error
        {
            self.errors.push(error);
        }
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
            let format = veta_core::convert::ImportFormat::from_path(&path);
            if format == Some(veta_core::convert::ImportFormat::Csv) {
                // Asks how to read it; one dialog at a time.
                if self.dialog.is_none() {
                    self.edit = None;
                    self.dialog = Some(Dialog::Import(ImportDialog::new(path)));
                }
                continue;
            }
            self.opening.push(path.clone());
            let worker_path = path.clone();
            let budget = options.memory_budget;
            if format == Some(veta_core::convert::ImportFormat::Json) {
                let label = format!("Importing {}", file_label(&path));
                tasks.push(self.start_transfer(
                    Some(path.clone()),
                    label,
                    move |progress| {
                        veta_core::convert::import_json(&worker_path, budget, progress)
                            .map(Loaded::new)
                            .map_err(|e| e.to_string())
                    },
                    move |result| Message::Opened(path.clone(), result),
                ));
                continue;
            }
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

    /// Adds a newly opened or imported document as the active tab.
    fn add_document(&mut self, path: &Path, mut document: Document) {
        if document.path().is_none() {
            // Imported data: save with the user's defaults for new files.
            document.set_initial_writer_settings(self.config.new_files.settings());
        }
        self.add_tab(document);
        self.config.add_recent(path);
        self.save_config();
    }

    fn add_tab(&mut self, document: Document) {
        let mut grid = GridView::new(&document, self.text_size);
        // Start on the first cell, at the top left.
        grid.apply(GridEvent::Select { row: 0, column: 0 }, &document);
        let id = self.workbook.add(document);
        self.tabs.push(Tab { id, grid });
        self.active = Some(id);
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
                let editor = edit.filter(|e| {
                    let selected = tab
                        .grid
                        .selected()
                        .map(|(r, c)| (r, doc.schema().fields().get(c).map(|f| f.name().clone())));
                    selected == Some((e.row, Some(e.column.clone())))
                });
                let editor = editor.map(|e| {
                    iced::widget::text_input("", &e.text)
                        .id(EDITOR_ID)
                        .on_input(Message::EditInput)
                        .on_submit(Message::CommitEdit)
                        .size(ui.size - 1.0)
                        .padding([1, 5])
                        .into()
                });
                let grid =
                    grid::grid(&tab.grid, ui.tokens, move |e| Message::Grid(id, e)).editor(editor);
                let grid: Element<'_, Message> =
                    match self.computing.as_ref().filter(|c| c.document == id) {
                        // Not opaque: the table can still be scrolled and
                        // read; edits are refused until the step is done.
                        Some(c) => stack![
                            grid,
                            panes::computing(computing_label(doc, c), c.progress, ui)
                        ]
                        .into(),
                        None => grid.into(),
                    };
                let mut grid_column = column![
                    panes::formula_bar(tab, doc, edit, ui),
                    divider(ui.tokens, false),
                ];
                if let Some(bar) = panes::filter_bar(tab, doc, ui) {
                    grid_column = grid_column.push(bar);
                }
                let grid =
                    grid_column.push(container(grid).width(Length::Fill).height(Length::Fill));
                if self.show_side_pane {
                    row![
                        grid,
                        divider(ui.tokens, true),
                        panes::side_pane(
                            tab,
                            doc,
                            self.side_tab,
                            self.stats_view(id),
                            panes::ValueView {
                                tree: self.value_tree(tab, doc),
                                collapsed: &self.collapsed,
                                leaf_edit: self
                                    .leaf_edit
                                    .as_ref()
                                    .filter(|e| e.document == id)
                                    .map(|e| panes::LeafEditView {
                                        pointer: e.pointer.as_slice(),
                                        text: e.text.as_str(),
                                        error: e.error.as_deref(),
                                        rename: e.rename,
                                    }),
                            },
                            ui
                        )
                    ]
                    .into()
                } else {
                    grid.into()
                }
            }
            None => panes::empty_state(!self.opening.is_empty(), &self.config.recent_files, ui),
        };

        let column = self.active_document().and_then(|(tab, doc)| {
            let c = tab.grid.selected_columns()?.start;
            doc.schema().fields().get(c).map(|f| f.data_type().clone())
        });
        let context = ribbon::Context {
            has_document: self.active.is_some(),
            has_selection: self.active.and_then(|id| self.selected_rows(id)).is_some(),
            has_cell: self
                .active_document()
                .is_some_and(|(tab, _)| matches!(tab.grid.selection(), Selection::Cell(..))),
            editable: self.active.is_some_and(|id| self.editable(id)),
            column: match column {
                None => ribbon::ColumnKind::None,
                Some(veta_core::arrow::datatypes::DataType::Struct(fields)) => {
                    ribbon::ColumnKind::Struct {
                        nested_structs: fields.iter().any(|f| {
                            matches!(
                                f.data_type(),
                                veta_core::arrow::datatypes::DataType::Struct(_)
                            )
                        }),
                    }
                }
                Some(
                    veta_core::arrow::datatypes::DataType::List(_)
                    | veta_core::arrow::datatypes::DataType::LargeList(_)
                    | veta_core::arrow::datatypes::DataType::FixedSizeList(..)
                    | veta_core::arrow::datatypes::DataType::Map(..),
                ) => ribbon::ColumnKind::List,
                Some(_) => ribbon::ColumnKind::Plain,
            },
            filter_active: self
                .active_document()
                .is_some_and(|(tab, doc)| tab.grid.filter_counts(doc).is_some()),
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
            panes::status_bar(
                self.active_document(),
                self.opening
                    .iter()
                    .filter(|p| !self
                        .transfers
                        .iter()
                        .any(|t| t.importing.as_ref() == Some(p)))
                    .count(),
                &self.transfers,
                self.notice.as_deref(),
                ui
            ),
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
            let column = match menu.target {
                MenuTarget::Column(c) => Some(c),
                MenuTarget::Cell => self
                    .tabs
                    .iter()
                    .find(|t| t.id == menu.document)
                    .and_then(|t| t.grid.selected())
                    .map(|(_, c)| c),
                _ => None,
            };
            let column_type = column.and_then(|c| {
                self.workbook
                    .get(menu.document)
                    .and_then(|d| d.schema().fields().get(c).map(|f| f.data_type().clone()))
            });
            let sections = panes::menu_sections(menu.target, rows, columns, column_type.as_ref());
            // Opens up or to the left where it wouldn't fit below or right.
            let height = panes::menu_height(&sections, ui);
            let mut position = menu.position;
            if position.y + height > self.window_size.height {
                position.y = (self.window_size.height - height).max(0.0);
            }
            if position.x + panes::MENU_WIDTH > self.window_size.width {
                position.x = (position.x - panes::MENU_WIDTH).max(0.0);
            }
            return stack![
                window,
                mouse_area(
                    container(iced::widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                )
                .on_press(Message::CloseMenu)
                .on_right_press(Message::CloseMenu),
                pin(panes::context_menu(sections, ui)).position(position),
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
            Dialog::Filter { dialog, .. } => dialog.view(ui.tokens).map(Message::Filter),
            Dialog::Sort { dialog, .. } => dialog.view(ui.tokens).map(Message::Sort),
            Dialog::Import(dialog) => dialog.view(ui.tokens).map(Message::Import),
            Dialog::Export { dialog, .. } => dialog.view(ui.tokens).map(Message::Export),
            Dialog::Cast { dialog, .. } => dialog.view(ui.tokens).map(Message::Cast),
            Dialog::Replace { dialog, .. } => dialog.view(ui.tokens).map(Message::Replace),
            Dialog::Format { dialog, .. } => dialog.view(ui.tokens).map(Message::Format),
            Dialog::Split { dialog, .. } => dialog.view(ui.tokens).map(Message::Split),
            Dialog::Custom { dialog, .. } => dialog.view(ui.tokens).map(Message::Custom),
            Dialog::Merge { dialog, .. } => dialog.view(ui.tokens).map(Message::Merge),
            Dialog::Combine { dialog, .. } => dialog.view(ui.tokens).map(Message::Combine),
            Dialog::ConfirmStep { .. } => settings::confirm_step(ui.tokens),
            Dialog::Choose { dialog, .. } => dialog.view(ui.tokens).map(Message::Choose),
            Dialog::Writer { dialog, .. } => dialog.view(ui.tokens).map(Message::Writer),
            Dialog::Metadata { dialog, .. } => dialog.view(ui.tokens).map(Message::Metadata),
            Dialog::Json { dialog, .. } => dialog.view(ui.tokens).map(Message::Json),
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

/// What the computing overlay says is running.
fn computing_label(doc: &Document, c: &Computing) -> String {
    match (c.target, doc.steps().get(c.step)) {
        (Target::Document, Some(step)) => format!("{}. {}", c.step + 1, step.describe()),
        _ => "Filtering rows".to_owned(),
    }
}

/// How to refer to a column in a formula.
fn formula_reference(column: &str) -> String {
    format!("[{column}]")
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

/// The file kind an export format writes.
fn export_kind(choice: dialogs::ExportChoice) -> FileKind {
    match choice {
        dialogs::ExportChoice::Csv => FileKind::Csv,
        dialogs::ExportChoice::JsonLines => FileKind::JsonLines,
        dialogs::ExportChoice::JsonArray => FileKind::Json,
    }
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
    use veta_core::arrow::datatypes::DataType;
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
        assert!(app.edit.as_ref().unwrap().error.is_some());
        assert!(
            app.errors.is_empty(),
            "shown next to the editor, not as a banner"
        );
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

        let _ = app.update(Message::Action(Action::DeleteRows));
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

        // With one column selected, the ribbon asks; the dialog keeps the
        // checked ones.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::KeepColumns));
        assert!(matches!(app.dialog, Some(Dialog::Choose { .. })));
        let _ = app.update(Message::Choose(ChooseMessage::Toggle(2, true)));
        let _ = app.update(Message::Choose(ChooseMessage::Apply));
        assert_eq!(names(&app), ["id", "name"]);

        // The column menu keeps the selection directly.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Column(1),
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::KeepColumns));
        assert!(app.dialog.is_none());
        assert_eq!(names(&app), ["name"]);

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
        let _ = app.update(Message::Action(Action::DeleteRows));
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
        let _ = app.update(Message::Action(Action::DeleteRows));
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
        let _ = app.update(Message::Action(Action::DeleteRows));

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
            let _ = app.update(Message::Action(Action::DeleteRows));
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
    fn applied_steps_preview_edit_and_warnings() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let doc = |app: &App| app.workbook.get(id).unwrap().steps().len();

        // Rename "name" → "label", then delete row 1.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::RenameColumn));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("label".into())));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectRows {
                row: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::DeleteRows));
        assert_eq!(doc(&app), 2);

        // Preview the source: read-only.
        let _ = app.update(Message::PreviewStep(id, 0));
        assert_eq!(app.tabs[0].grid.preview(), Some(0));
        assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("0")));
        let _ = app.update(Message::Action(Action::DeleteRows));
        assert_eq!(doc(&app), 2, "changes are disabled while previewing");
        assert_eq!(app.errors.len(), 1);
        let _ = app.update(Message::PreviewStep(id, usize::MAX));
        assert_eq!(app.tabs[0].grid.preview(), None);

        // Editing the rename asks first, because a later step deletes rows
        // by position.
        let _ = app.update(Message::EditStep(0));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("title".into())));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
        assert!(matches!(app.dialog, Some(Dialog::ConfirmStep { .. })));
        let _ = app.update(Message::ConfirmStep(true));
        let schema = app.workbook.get(id).unwrap().schema();
        assert_eq!(schema.field(1).name(), "title");

        // Removing the last step doesn't ask.
        let _ = app.update(Message::RemoveStep(1));
        assert!(app.dialog.is_none());
        assert_eq!(doc(&app), 1);
    }

    /// Runs the pending pass of document `id` here, as the worker would.
    fn finish_compute(app: &mut App, id: DocumentId) {
        let job = app.workbook.get(id).unwrap().compute_job().unwrap();
        let key = job.key().to_vec();
        let result = job
            .run(&mut |_| true)
            .map(Slot::new)
            .map_err(|e| e.to_string());
        let _ = app.update(Message::ComputeDone(id, Target::Document, key, result));
    }

    /// Runs the pending filter of document `id`'s tab here.
    fn finish_filter(app: &mut App, id: DocumentId) {
        let tab = app.tabs.iter().find(|t| t.id == id).unwrap();
        let job = tab.grid.filter_job().unwrap();
        let key = job.key().to_vec();
        let result = job
            .run(&mut |_| true)
            .map(Slot::new)
            .map_err(|e| e.to_string());
        let _ = app.update(Message::ComputeDone(id, Target::View, key, result));
    }

    #[test]
    fn filter_view_then_keep_or_remove() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let n = fixtures::MIXED_SETTINGS_ROWS;
        let rows = |app: &App| app.workbook.get(id).unwrap().num_rows();
        let steps = |app: &App| app.workbook.get(id).unwrap().steps().len();
        let shown = |app: &App| {
            let doc = app.workbook.get(id).unwrap();
            app.tabs[0].grid.filter_counts(doc).map(|c| c.0)
        };
        let anas = (0..n).filter(|i| i % 5 == 0 && i % 7 != 0).count();

        // Filter opens on the selected column and sets a view, not a step.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::FilterRows));
        let _ = app.update(Message::Filter(FilterMessage::Op(0, FilterOp::Greater)));
        let _ = app.update(Message::Filter(FilterMessage::Column(0, "score".into())));
        let _ = app.update(Message::Filter(FilterMessage::Value(0, "abc".into())));
        let _ = app.update(Message::Filter(FilterMessage::Apply));
        assert!(
            matches!(app.dialog, Some(Dialog::Filter { .. })),
            "an invalid value keeps the dialog open"
        );
        let _ = app.update(Message::Filter(FilterMessage::Column(0, "name".into())));
        let _ = app.update(Message::Filter(FilterMessage::Op(0, FilterOp::Equals)));
        let _ = app.update(Message::Filter(FilterMessage::Value(0, "ANA".into())));
        let _ = app.update(Message::Filter(FilterMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(steps(&app), 0);
        assert!(
            app.computing
                .as_ref()
                .is_some_and(|c| c.target == Target::View)
        );
        finish_filter(&mut app, id);
        assert_eq!(shown(&app), Some(anas));
        assert_eq!(app.tabs[0].grid.value(0, 1), Some(Some("ana")));

        // Read-only while filtered.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 0 }));
        let _ = app.update(Message::Action(Action::DeleteRows));
        assert_eq!(steps(&app), 0);
        assert!(!app.errors.is_empty());
        app.errors.clear();

        // Filter by a cell value adds to the filter.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 3 }));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Cell,
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::ExcludeValue));
        assert_eq!(app.tabs[0].grid.filter().unwrap().conditions.len(), 2);
        finish_filter(&mut app, id);
        let first_flag = app.tabs[0].grid.value(0, 3).flatten().map(str::to_owned);
        assert!(first_flag.is_some());

        // Remove rows: the inverse of what's shown becomes a step.
        let shown_now = shown(&app).unwrap();
        let _ = app.update(Message::Action(Action::RemoveRows));
        assert!(app.tabs[0].grid.filter().is_none());
        finish_compute(&mut app, id);
        assert_eq!(rows(&app), n - shown_now);
        let _ = app.update(Message::Action(Action::Undo));
        assert_eq!(rows(&app), n);

        // Clear drops the view; Keep keeps the rows shown.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 1, column: 1 }));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Cell,
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::KeepValue));
        finish_filter(&mut app, id);
        let _ = app.update(Message::ClearFilter);
        assert!(app.tabs[0].grid.filter().is_none());
        assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("0")));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Cell,
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::KeepValue));
        finish_filter(&mut app, id);
        let _ = app.update(Message::Action(Action::KeepRows));
        finish_compute(&mut app, id);
        let bos = (0..n).filter(|i| i % 5 == 1 && i % 7 != 0).count();
        assert_eq!(rows(&app), bos);

        // Cancel while filtering clears the filter.
        let _ = app.update(Message::Action(Action::FilterRows));
        let _ = app.update(Message::Filter(FilterMessage::Value(0, "x".into())));
        let _ = app.update(Message::Filter(FilterMessage::Apply));
        assert!(app.computing.is_some());
        let _ = app.update(Message::CancelCompute);
        assert!(app.tabs[0].grid.filter().is_none());
        assert_eq!(steps(&app), 1);

        // Editing the filter step still works through the dialog.
        let _ = app.update(Message::EditStep(0));
        let _ = app.update(Message::Filter(FilterMessage::Value(0, "ana".into())));
        let _ = app.update(Message::Filter(FilterMessage::Apply));
        finish_compute(&mut app, id);
        assert_eq!(rows(&app), anas);
    }

    #[test]
    fn no_edits_while_computing_but_other_files_work() {
        let dir = TempDir::new();
        let a = dir.join("a.parquet");
        let b = dir.join("b.parquet");
        fixtures::mixed_settings(&a);
        fixtures::mixed_settings(&b);
        let mut app = app_with(&[&a, &b]);
        let (ida, idb) = (app.tabs[0].id, app.tabs[1].id);
        let _ = app.update(Message::SelectTab(ida));
        let _ = app.update(Message::Grid(ida, GridEvent::Select { row: 0, column: 0 }));
        let _ = app.update(Message::Action(Action::SortDescending));
        let steps = |app: &App, id| app.workbook.get(id).unwrap().steps().len();
        assert_eq!(steps(&app, ida), 1);

        let attempts = [
            Message::Grid(
                ida,
                GridEvent::StartEdit {
                    initial: Some("5".into()),
                },
            ),
            Message::EditInput("5".into()),
            Message::Grid(ida, GridEvent::ClearCell),
            Message::Grid(ida, GridEvent::DeleteRows),
            Message::Grid(ida, GridEvent::InsertRows),
            Message::Grid(ida, GridEvent::MoveColumn { from: 0, to: 1 }),
            Message::Action(Action::DeleteRows),
            Message::Action(Action::RemoveColumns),
            Message::Action(Action::SortAscending),
            Message::Action(Action::Undo),
            Message::RemoveStep(0),
            Message::EditStep(0),
            Message::LoadRecipe,
        ];
        for message in attempts {
            let _ = app.update(message.clone());
            assert!(app.edit.is_none(), "{message:?}");
            assert!(app.dialog.is_none(), "{message:?}");
            assert_eq!(steps(&app, ida), 1, "{message:?}");
            assert!(app.computing.is_some(), "{message:?}");
        }
        // The column menu's actions are refused too.
        let _ = app.update(Message::Grid(
            ida,
            GridEvent::ContextMenu {
                target: MenuTarget::Column(0),
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::RemoveColumns));
        assert_eq!(steps(&app, ida), 1);

        // The other file can be edited meanwhile.
        let _ = app.update(Message::SelectTab(idb));
        let _ = app.update(Message::Grid(idb, GridEvent::Select { row: 0, column: 0 }));
        let _ = app.update(Message::Action(Action::RemoveColumns));
        assert!(matches!(app.dialog, Some(Dialog::Choose { .. })));
        let _ = app.update(Message::CloseDialog);
        let _ = app.update(Message::Grid(idb, GridEvent::Select { row: 1, column: 1 }));
        let _ = app.update(Message::Grid(idb, GridEvent::ClearCell));
        assert_eq!(steps(&app, idb), 1);

        let _ = app.update(Message::SelectTab(ida));
        finish_compute(&mut app, ida);
        let _ = app.update(Message::Action(Action::Undo));
        assert_eq!(steps(&app, ida), 0);
    }

    /// Runs the pending statistics pass here, as the worker would.
    fn finish_stats(app: &mut App) {
        let run = app.stats.running.as_ref().unwrap();
        let (id, revision) = (run.document, run.revision);
        let job = app
            .workbook
            .get(id)
            .unwrap()
            .stats_job(run.columns.clone())
            .unwrap();
        let result = job.run(&mut |_| true).map_err(|e| e.to_string());
        let _ = app.update(Message::StatsDone(id, revision, result));
    }

    #[test]
    fn statistics_pane_follows_selection_and_edits() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;

        // Hidden: nothing computed.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 0 }));
        assert!(app.stats.running.is_none());

        let _ = app.update(Message::Action(Action::StatisticsPane));
        assert_eq!(app.side_tab, SideTab::Stats);
        // Quick stats from the footer while the pass runs.
        let view = app.stats_view(id);
        assert_eq!(view.columns.len(), 1);
        let (name, quick, partial) = &view.columns[0];
        assert_eq!(name, "id");
        assert!(*partial);
        assert_eq!(quick.as_ref().unwrap().max.as_deref(), Some("999"));
        assert_eq!(app.stats.running.as_ref().unwrap().columns, ["id"]);
        finish_stats(&mut app);
        let view = app.stats_view(id);
        let (_, full, partial) = &view.columns[0];
        assert!(!partial);
        assert_eq!(full.as_ref().unwrap().distinct, Some(1000));

        // Selecting more columns computes only the missing ones.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: true,
            },
        ));
        assert_eq!(app.stats.running.as_ref().unwrap().columns, ["name"]);
        finish_stats(&mut app);
        assert!(app.stats.running.is_none());

        // After an edit the statistics are recomputed (once the step is).
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
        let _ = app.update(Message::Action(Action::DeleteRows));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 0,
                extend: false,
            },
        ));
        finish_stats(&mut app);
        let view = app.stats_view(id);
        assert_eq!(view.columns[0].1.as_ref().unwrap().count, 999);
        assert!(
            app.stats
                .cache
                .keys()
                .all(|(_, r, _)| *r == app.workbook.get(id).unwrap().revision())
        );

        // Switching tabs stops a pass.
        let _ = app.update(Message::Action(Action::Undo));
        assert!(app.stats.running.is_some());
        let _ = app.update(Message::SideTab(SideTab::Steps));
        assert!(app.stats.running.is_none());
    }

    #[test]
    fn import_csv_and_json_then_save_as_parquet() {
        let dir = TempDir::new();
        let csv = dir.join("eu.csv");
        std::fs::write(&csv, "id;price\n1;1,5\n2;2,25\n").unwrap();
        let mut app = app();
        let _ = app.update(Message::ImportPicked(Some(csv.clone())));
        let Some(Dialog::Import(dialog)) = &app.dialog else {
            panic!("CSV files ask how to read them");
        };
        // Semicolons were detected, so decimal commas are assumed.
        let options = dialog.options();
        assert_eq!((options.delimiter, options.decimal), (b';', ','));
        let _ = app.update(Message::Import(ImportMessage::Type(
            0,
            dialogs::CastTarget::Text,
        )));
        let _ = app.update(Message::Import(ImportMessage::Import));
        assert!(app.dialog.is_none());
        assert_eq!(app.opening, std::slice::from_ref(&csv));
        // The worker's result:
        let mut options = options;
        options.types = Some(vec![DataType::Utf8, DataType::Float64]);
        let doc = veta_core::convert::import_csv(&csv, &options, 1 << 30, &mut |_| true).unwrap();
        let _ = app.update(Message::Imported(csv.clone(), Ok(Slot::new(doc))));
        let id = app.tabs[0].id;
        let doc = app.workbook.get(id).unwrap();
        assert_eq!(doc.title(), "eu.csv");
        assert!(doc.is_modified());
        assert_eq!(app.tabs[0].grid.value(1, 1), Some(Some("2.25")));
        assert_eq!(
            files::suggested_name(&doc.title(), FileKind::Parquet),
            "eu.parquet"
        );

        // JSON opens directly in the background.
        let json = dir.join("a.json");
        std::fs::write(&json, "[{\"x\": 1}]").unwrap();
        let _ = app.update(Message::FilesPicked(vec![json.clone()]));
        assert!(app.dialog.is_none());
        assert!(app.opening.contains(&json));
    }

    #[test]
    fn csv_quote_and_custom_separator() {
        let dir = TempDir::new();
        let csv = dir.join("q.txt");
        std::fs::write(&csv, "a#b\n'x#y'#2\n").unwrap();
        let mut dialog = ImportDialog::new(csv);
        dialog.update(ImportMessage::Delimiter(dialogs::Delimiter::Other));
        dialog.update(ImportMessage::Other("ab".into()));
        assert!(
            dialog.update(ImportMessage::Import).is_none(),
            "bad separator"
        );
        dialog.update(ImportMessage::Other("#".into()));
        dialog.update(ImportMessage::Quote(dialogs::Quote::Single));
        let options = dialog.update(ImportMessage::Import).unwrap();
        assert_eq!((options.delimiter, options.quote), (b'#', Some(b'\'')));
    }

    #[test]
    fn import_reports_values_that_did_not_fit() {
        let dir = TempDir::new();
        let csv = dir.join("bad.csv");
        std::fs::write(&csv, "n\n1\nx\n").unwrap();
        let mut app = app();
        let _ = app.update(Message::ImportPicked(Some(csv.clone())));
        let _ = app.update(Message::Import(ImportMessage::Type(
            0,
            dialogs::CastTarget::Int64,
        )));
        let Some(Dialog::Import(dialog)) = &app.dialog else {
            panic!("import dialog");
        };
        let options = dialog.options();
        assert_eq!(options.on_error, OnError::ErrorColumn);
        let result = veta_core::convert::import_csv(&csv, &options, 1 << 30, &mut |_| true)
            .map(Slot::new)
            .map_err(|e| e.to_string());
        let _ = app.update(Message::Imported(csv, result));
        assert_eq!(app.tabs.len(), 1);
        assert!(app.errors[0].contains("n (1)"), "{:?}", app.errors);
        assert_eq!(app.tabs[0].grid.value(1, 1), Some(Some("x")));
    }

    #[test]
    fn nested_values_in_formula_bar_and_value_tab() {
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 1 }));
        assert_eq!(app.tabs[0].grid.value(0, 1), Some(Some("{x: 1, y: 2}")));
        assert_eq!(
            app.tabs[0].grid.edit_value(0, 1),
            Some(Some(r#"{"x":1,"y":2}"#))
        );
        assert_eq!(app.cell_text(id).as_deref(), Some(r#"{"x":1,"y":2}"#));
        let _ = app.update(Message::SideTab(SideTab::Value));
        let tab = &app.tabs[0];
        let doc = app.workbook.get(id).unwrap();
        let tree = app.value_tree(tab, doc).unwrap();
        assert_eq!((tree.label.as_str(), tree.children.len()), ("point", 2));
        let _ = app.update(Message::ToggleNode("point".into()));
        assert!(app.collapsed.contains("point"));
    }

    #[test]
    fn struct_columns_expand_into_editable_fields() {
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let grid = |app: &mut App, event| {
            let _ = app.update(Message::Grid(id, event));
        };
        let cell = |app: &App, row: usize| {
            let doc = app.workbook.get(id).unwrap();
            veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][1]
                .clone()
        };

        grid(&mut app, GridEvent::Expand { column: 1 });
        let g = &app.tabs[0].grid;
        assert_eq!(g.value(0, 1), Some(Some("1")));
        assert_eq!(g.value(0, 2), Some(Some("2")));
        assert_eq!(g.value(2, 1), Some(None), "null struct, null fields");
        assert_eq!(g.value(0, 3), Some(Some("[2 items]")), "tags moved right");

        // A field cell edits that field of the struct.
        grid(&mut app, GridEvent::Select { row: 0, column: 2 });
        assert_eq!(app.tabs[0].grid.selected(), Some((0, 1)));
        grid(&mut app, GridEvent::StartEdit { initial: None });
        assert_eq!(app.edit.as_ref().unwrap().text, "2");
        let _ = app.update(Message::EditInput("20".into()));
        let _ = app.update(Message::CommitEdit);
        assert_eq!(cell(&app, 0).as_deref(), Some("{x: 1, y: 20}"));
        grid(&mut app, GridEvent::Select { row: 2, column: 2 });
        grid(&mut app, GridEvent::StartEdit { initial: None });
        let _ = app.update(Message::EditInput("5".into()));
        let _ = app.update(Message::CommitEdit);
        assert_eq!(cell(&app, 2).as_deref(), Some("{x: null, y: 5}"));
        grid(&mut app, GridEvent::Select { row: 2, column: 2 });
        grid(&mut app, GridEvent::ClearCell);
        assert_eq!(cell(&app, 2).as_deref(), Some("{x: null, y: null}"));

        // Column commands see document columns.
        grid(
            &mut app,
            GridEvent::SelectColumns {
                column: 2,
                extend: false,
            },
        );
        assert_eq!(app.tabs[0].grid.selected_columns(), Some(1..2));
        app.tabs[0].grid.select_columns(2..3);
        assert_eq!(app.tabs[0].grid.selection(), Selection::Columns(2..3));

        // Nested structs expand level by level; collapsing restores.
        grid(&mut app, GridEvent::Expand { column: 6 });
        grid(&mut app, GridEvent::Expand { column: 7 });
        let g = &app.tabs[0].grid;
        assert_eq!(g.value(0, 6), Some(Some("Ana")));
        assert_eq!(g.value(0, 8), Some(Some("6")), "owner.home.y");
        grid(&mut app, GridEvent::Collapse { column: 7 });
        grid(&mut app, GridEvent::Collapse { column: 1 });
        let g = &app.tabs[0].grid;
        assert_eq!(g.value(0, 1), Some(Some("{x: 1, y: 20}")));
        assert_eq!(g.value(0, 5), Some(Some("Ana")), "owner still expanded");
    }

    #[test]
    fn combine_columns_into_a_struct() {
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
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
        let select = |app: &mut App, column, extend| {
            let _ = app.update(Message::Grid(
                id,
                GridEvent::SelectColumns { column, extend },
            ));
        };
        // Expanded fields go back into their struct by default.
        select(&mut app, 1, false);
        let _ = app.update(Message::Action(Action::ExpandStruct));
        select(&mut app, 1, false);
        select(&mut app, 2, true);
        let _ = app.update(Message::Action(Action::CombineStruct));
        let _ = app.update(Message::Combine(CombineMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(names(&app)[..3], ["id", "point", "tags"]);

        // Other columns get a new name; a taken one keeps the dialog open.
        select(&mut app, 0, false);
        select(&mut app, 2, true);
        let _ = app.update(Message::Action(Action::CombineStruct));
        let _ = app.update(Message::Combine(CombineMessage::Name("owner".into())));
        let _ = app.update(Message::Combine(CombineMessage::Apply));
        assert!(matches!(app.dialog, Some(Dialog::Combine { .. })));
        let _ = app.update(Message::Combine(CombineMessage::Name("rec".into())));
        let _ = app.update(Message::Combine(CombineMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(names(&app), ["rec", "path", "scores", "owner"]);

        // The step reopens in its dialog.
        let _ = app.update(Message::EditStep(2));
        assert!(matches!(app.dialog, Some(Dialog::Combine { .. })));
    }

    #[test]
    fn edit_nested_values_and_expand_explode() {
        use veta_core::display::PathPart;
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let cell = |app: &App, row: usize, column: &str| {
            let doc = app.workbook.get(id).unwrap();
            let i = doc.schema().index_of(column).unwrap();
            veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][i]
                .clone()
        };

        // Edit owner.home.y from the Value tab.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 5 }));
        let _ = app.update(Message::SideTab(SideTab::Value));
        let pointer = vec![PathPart::Field("home".into()), PathPart::Field("y".into())];
        let int64 = veta_core::arrow::datatypes::DataType::Int64;
        let _ = app.update(Message::EditLeaf(
            pointer.clone(),
            int64.clone(),
            "6".into(),
        ));
        let _ = app.update(Message::LeafInput("not a number".into()));
        let _ = app.update(Message::CommitLeaf);
        assert!(app.leaf_edit.as_ref().unwrap().error.is_some());
        let _ = app.update(Message::LeafInput("60".into()));
        let _ = app.update(Message::CommitLeaf);
        assert!(app.leaf_edit.is_none());
        assert_eq!(
            cell(&app, 0, "owner").as_deref(),
            Some("{name: Ana, home: {x: 5, y: 60}}")
        );

        // Whole cells edit as JSON in the cell editor.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 1, column: 2 }));
        let _ = app.update(Message::Grid(id, GridEvent::StartEdit { initial: None }));
        assert_eq!(app.edit.as_ref().unwrap().text, "[]");
        let _ = app.update(Message::EditInput(r#"["x", "y"]"#.into()));
        let _ = app.update(Message::CommitEdit);
        assert_eq!(cell(&app, 1, "tags").as_deref(), Some("[x, y]"));

        // Column menu: expand the struct, explode the list.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Column(1),
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::ExpandStruct));
        assert_eq!(cell(&app, 0, "point.x").as_deref(), Some("1"));
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 3,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::ExplodeList));
        finish_compute(&mut app, id);
        // Row 1 now has two tags: 2 + 2 + 1 (null row) + 3.
        assert_eq!(app.workbook.get(id).unwrap().num_rows(), 8);
    }

    #[test]
    fn export_dialog() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let _ = app.update(Message::Action(Action::Export));
        let _ = app.update(Message::Export(ExportMessage::Delimiter(
            dialogs::Delimiter::Semicolon,
        )));
        let Some(Dialog::Export { dialog, .. }) = &app.dialog else {
            panic!("export asks for a format");
        };
        assert_eq!(
            dialog.format(),
            veta_core::convert::ExportFormat::Csv(veta_core::convert::CsvExportOptions {
                delimiter: b';',
                decimal: ',',
                ..Default::default()
            })
        );
        let _ = app.update(Message::ExportTo(id, Some(dir.join("out.csv"))));
        assert!(app.dialog.is_none());
        assert_eq!(app.transfers.len(), 1);
        let transfer = app.transfers[0].id;
        let _ = app.update(Message::TransferProgress(transfer, 0.5));
        assert_eq!(app.transfers[0].progress, 0.5);
        let _ = app.update(Message::CancelTransfer(transfer));
        assert!(app.transfers[0].cancel.load(Ordering::Relaxed));
        let _ = app.update(Message::Exported(Ok(dir.join("out.csv"))));
        let _ = app.update(Message::TransferDone(transfer));
        assert!(app.transfers.is_empty());
        assert!(app.notice.as_deref().unwrap().starts_with("Exported"));
    }

    #[test]
    fn keep_selected_rows_from_the_menu() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
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
        let _ = app.update(Message::Grid(
            id,
            GridEvent::ContextMenu {
                target: MenuTarget::Rows,
                position: iced::Point::ORIGIN,
            },
        ));
        let _ = app.update(Message::Menu(MenuItem::KeepRows));
        assert_eq!(app.workbook.get(id).unwrap().num_rows(), 3);
        assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("2")));
    }

    #[test]
    fn sort_from_ribbon_and_edit_step() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let id_at = |app: &App, row: usize| {
            let doc = app.workbook.get(id).unwrap();
            veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][0]
                .clone()
        };

        // Opening a file starts on the first cell (#90).
        assert_eq!(app.tabs[0].grid.selected(), Some((0, 0)));
        let _ = app.update(Message::Action(Action::SortDescending));
        finish_compute(&mut app, id);
        assert_eq!(id_at(&app, 0).as_deref(), Some("999"));

        // Custom sort: score descending with nulls first, as a new step.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
        let _ = app.update(Message::Action(Action::CustomSort));
        let _ = app.update(Message::Sort(SortMessage::Direction(
            0,
            dialogs::Direction::Descending,
        )));
        let _ = app.update(Message::Sort(SortMessage::Nulls(0, dialogs::Nulls::First)));
        let _ = app.update(Message::Sort(SortMessage::Apply));
        finish_compute(&mut app, id);
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 2);
        let doc = app.workbook.get(id).unwrap();
        let first = veta_core::display::format_batch(&doc.read(0..1).unwrap()).unwrap();
        assert_eq!(first[0][2], None, "nulls first");
        let _ = app.update(Message::Action(Action::Undo));

        // Edit it into a two-key sort: flag, then id ascending.
        let _ = app.update(Message::EditStep(0));
        assert!(matches!(app.dialog, Some(Dialog::Sort { .. })));
        let _ = app.update(Message::Sort(SortMessage::Direction(
            0,
            dialogs::Direction::Ascending,
        )));
        let _ = app.update(Message::Sort(SortMessage::Add));
        let _ = app.update(Message::Sort(SortMessage::Column(0, "flag".into())));
        let _ = app.update(Message::Sort(SortMessage::Column(1, "id".into())));
        let _ = app.update(Message::Sort(SortMessage::Apply));
        assert!(app.dialog.is_none());
        finish_compute(&mut app, id);
        assert_eq!(id_at(&app, 0).as_deref(), Some("1"));
        assert_eq!(id_at(&app, 500).as_deref(), Some("0"));
    }

    #[test]
    fn change_type_dialog_and_edit_step() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let schema = |app: &App| app.workbook.get(id).unwrap().schema();

        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::DataType));
        assert!(matches!(app.dialog, Some(Dialog::Cast { .. })));
        let _ = app.update(Message::Cast(CastMessage::Type(dialogs::CastTarget::Int64)));
        let _ = app.update(Message::Cast(CastMessage::OnError(
            veta_core::steps::OnError::ErrorColumn,
        )));
        let _ = app.update(Message::Cast(CastMessage::Apply));
        assert!(app.dialog.is_none());
        // The check finds errors, so it's dropped and the errors kept.
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 2);
        finish_compute(&mut app, id);
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 1);
        assert_eq!(schema(&app).field(1).data_type(), &DataType::Int64);
        assert_eq!(schema(&app).field(2).name(), "name errors");
        assert!(
            app.errors
                .iter()
                .any(|e| e.contains("kept in column name errors"))
        );

        // Without errors the error column is removed.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::DataType));
        let _ = app.update(Message::Cast(CastMessage::Type(dialogs::CastTarget::Text)));
        let _ = app.update(Message::Cast(CastMessage::OnError(
            veta_core::steps::OnError::ErrorColumn,
        )));
        let _ = app.update(Message::Cast(CastMessage::Apply));
        finish_compute(&mut app, id);
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 3);
        assert_eq!(schema(&app).fields().len(), 5);
        let _ = app.update(Message::Action(Action::Undo));
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 1);

        // Edit it: back to null on error, so the error column goes away.
        let _ = app.update(Message::EditStep(0));
        let _ = app.update(Message::Cast(CastMessage::OnError(
            veta_core::steps::OnError::Null,
        )));
        let _ = app.update(Message::Cast(CastMessage::Apply));
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 1);
        assert_eq!(schema(&app).field(2).name(), "score");
    }

    #[test]
    fn replace_fill_and_format() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let cell = |app: &App, row: usize, column: usize| {
            let doc = app.workbook.get(id).unwrap();
            veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][column]
                .clone()
        };
        let select_column = |app: &mut App, column: usize| {
            let _ = app.update(Message::Grid(
                id,
                GridEvent::SelectColumns {
                    column,
                    extend: false,
                },
            ));
        };

        // Replace in score: an invalid number keeps the dialog open.
        select_column(&mut app, 2);
        let _ = app.update(Message::Action(Action::ReplaceValues));
        let _ = app.update(Message::Replace(ReplaceMessage::Find("x".into())));
        let _ = app.update(Message::Replace(ReplaceMessage::With("1".into())));
        let _ = app.update(Message::Replace(ReplaceMessage::Apply));
        assert!(matches!(app.dialog, Some(Dialog::Replace { .. })));
        let _ = app.update(Message::Replace(ReplaceMessage::FindNull(true)));
        let _ = app.update(Message::Replace(ReplaceMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(cell(&app, 0, 2).as_deref(), Some("1.0"));

        // Fill down names; uppercase them.
        select_column(&mut app, 1);
        let _ = app.update(Message::Action(Action::FillDown));
        finish_compute(&mut app, id);
        assert_eq!(cell(&app, 7, 1).as_deref(), Some("bo"));
        let _ = app.update(Message::Action(Action::FormatText));
        let _ = app.update(Message::Format(FormatMessage::Op(TextOp::Upper)));
        let _ = app.update(Message::Format(FormatMessage::Apply));
        assert_eq!(cell(&app, 7, 1).as_deref(), Some("BO"));

        // Edit the replace step: later steps aren't positional, no warning.
        let _ = app.update(Message::EditStep(0));
        let _ = app.update(Message::Replace(ReplaceMessage::With("2".into())));
        let _ = app.update(Message::Replace(ReplaceMessage::Apply));
        assert!(app.dialog.is_none());
        finish_compute(&mut app, id);
        assert_eq!(cell(&app, 0, 2).as_deref(), Some("2.0"));
        assert_eq!(app.workbook.get(id).unwrap().steps().len(), 3);
    }

    #[test]
    fn split_and_merge_columns() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let names = |app: &App| {
            let schema = app.workbook.get(id).unwrap().schema();
            schema
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect::<Vec<_>>()
        };

        // Merge id and name with "-".
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::MergeColumns));
        assert!(app.dialog.is_none(), "one column can't be merged");
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: true,
            },
        ));
        let _ = app.update(Message::Action(Action::MergeColumns));
        let _ = app.update(Message::Merge(MergeMessage::Separator("-".into())));
        let _ = app.update(Message::Merge(MergeMessage::Name("score".into())));
        let _ = app.update(Message::Merge(MergeMessage::Apply));
        assert!(
            matches!(app.dialog, Some(Dialog::Merge { .. })),
            "a clashing name keeps the dialog open"
        );
        let _ = app.update(Message::Merge(MergeMessage::Name("key".into())));
        let _ = app.update(Message::Merge(MergeMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(names(&app), ["key", "score", "flag"]);

        // Split it back; the dialog suggests 2 parts.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::SplitColumn));
        let _ = app.update(Message::Split(SplitMessage::Delimiter("-".into())));
        let _ = app.update(Message::Split(SplitMessage::Apply));
        assert_eq!(names(&app), ["key.1", "key.2", "score", "flag"]);

        // Edit the split: by position.
        let _ = app.update(Message::EditStep(1));
        let _ = app.update(Message::Split(SplitMessage::Mode(
            dialogs::SplitMode::Positions,
        )));
        let _ = app.update(Message::Split(SplitMessage::Positions("1, 2".into())));
        let _ = app.update(Message::Split(SplitMessage::Apply));
        assert_eq!(names(&app), ["key.1", "key.2", "key.3", "score", "flag"]);
    }

    #[test]
    fn custom_duplicate_and_index_columns() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let schema = |app: &App| app.workbook.get(id).unwrap().schema();
        let names = |app: &App| {
            schema(app)
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect::<Vec<_>>()
        };

        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 2,
                extend: false,
            },
        ));
        let _ = app.update(Message::Action(Action::Standard));
        let _ = app.update(Message::Custom(CustomMessage::Expression(
            "[score] * \"x\"".into(),
        )));
        let _ = app.update(Message::Custom(CustomMessage::Apply));
        assert!(
            matches!(app.dialog, Some(Dialog::Custom { .. })),
            "an invalid formula keeps the dialog open"
        );
        let _ = app.update(Message::Custom(CustomMessage::Expression(
            "score * 2".into(),
        )));
        let _ = app.update(Message::Custom(CustomMessage::Name("double".into())));
        let _ = app.update(Message::Custom(CustomMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(names(&app), ["id", "name", "score", "double", "flag"]);

        let _ = app.update(Message::Action(Action::DuplicateColumn));
        let _ = app.update(Message::Action(Action::DuplicateColumn));
        assert_eq!(names(&app)[3], "score - Copy 2");
        assert_eq!(schema(&app).field(3).data_type(), &DataType::Float64);
        let _ = app.update(Message::Action(Action::IndexColumn));
        assert_eq!(names(&app).last().map(String::as_str), Some("Index"));

        // Edit the first custom column.
        let _ = app.update(Message::EditStep(0));
        let _ = app.update(Message::Custom(CustomMessage::Expression(
            "text(score)".into(),
        )));
        let _ = app.update(Message::Custom(CustomMessage::Apply));
        assert_eq!(schema(&app).field(3 + 2).data_type(), &DataType::Utf8);
    }

    #[test]
    fn save_and_load_recipe() {
        let dir = TempDir::new();
        let a = dir.join("a.parquet");
        let b = dir.join("b.parquet");
        fixtures::mixed_settings(&a);
        fixtures::mixed_settings(&b);
        let mut app = app_with(&[&a, &b]);
        let (ida, idb) = (app.tabs[0].id, app.tabs[1].id);

        let _ = app.update(Message::SelectTab(ida));
        let _ = app.update(Message::Grid(ida, GridEvent::Select { row: 0, column: 0 }));
        let _ = app.update(Message::Action(Action::SortDescending));
        finish_compute(&mut app, ida);
        let _ = app.update(Message::Action(Action::IndexColumn));
        let recipe = dir.join("r-recipe.json");
        let _ = app.update(Message::SaveRecipeTo(ida, Some(recipe.clone())));
        assert!(app.errors.is_empty(), "{:?}", app.errors);

        let _ = app.update(Message::SelectTab(idb));
        let _ = app.update(Message::LoadRecipeFrom(idb, Some(recipe)));
        assert_eq!(app.workbook.get(idb).unwrap().steps().len(), 2);
        finish_compute(&mut app, idb);
        let first = |app: &App, id| {
            let doc = app.workbook.get(id).unwrap();
            veta_core::display::format_batch(&doc.read(0..1).unwrap()).unwrap()
        };
        assert_eq!(first(&app, ida), first(&app, idb));

        let _ = app.update(Message::LoadRecipeFrom(idb, Some(dir.join("missing.toml"))));
        assert_eq!(app.errors.len(), 1);
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

    #[test]
    fn new_file_takes_columns_and_rows() {
        let mut app = app();
        let _ = app.update(Message::Action(Action::New));
        let _ = app.update(Message::Action(Action::New));
        let titles: Vec<String> = app.workbook.iter().map(|(_, d)| d.title()).collect();
        assert_eq!(titles, ["Untitled", "Untitled 2"]);
        let id = app.active.unwrap();
        assert!(!app.workbook.get(id).unwrap().is_modified());

        let blank = |app: &mut App| {
            app.menu = Some(ContextMenu {
                document: id,
                target: MenuTarget::Blank,
                position: iced::Point::ORIGIN,
            });
        };
        blank(&mut app);
        let _ = app.update(Message::Menu(MenuItem::AddColumn));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("a".into())));
        let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
        blank(&mut app);
        let _ = app.update(Message::Menu(MenuItem::AddRow));
        blank(&mut app);
        let _ = app.update(Message::Menu(MenuItem::AddRow));
        let doc = app.workbook.get(id).unwrap();
        assert_eq!((doc.num_rows(), doc.num_columns()), (2, 1));
        assert_eq!(app.selected_rows(id), Some(1..2));
        assert!(doc.path().is_none(), "Save asks for a path");
    }

    #[test]
    fn rows_before_columns_in_a_new_file() {
        let mut app = app();
        let _ = app.update(Message::Action(Action::New));
        let id = app.active.unwrap();
        app.menu = Some(ContextMenu {
            document: id,
            target: MenuTarget::Blank,
            position: iced::Point::ORIGIN,
        });
        let _ = app.update(Message::Menu(MenuItem::AddRow));
        let _ = app.update(Message::Action(Action::InsertRows));
        let _ = app.update(Message::Action(Action::DeleteRows));
        let _ = app.view();
        assert_eq!(app.workbook.get(id).unwrap().num_rows(), 1);
    }

    #[test]
    fn nested_items_keys_and_json_editor() {
        use veta_core::display::PathPart;
        use veta_core::values::NestedEdit;
        let dir = TempDir::new();
        let path = dir.join("n.parquet");
        fixtures::nested(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let cell = |app: &App, row: usize, column: &str| {
            let doc = app.workbook.get(id).unwrap();
            let i = doc.schema().index_of(column).unwrap();
            veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][i]
                .clone()
        };
        // tags = [a, b]: add an item, remove the first.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
        let _ = app.update(Message::NestedEdit(vec![], NestedEdit::AppendItem));
        assert_eq!(cell(&app, 0, "tags").as_deref(), Some("[a, b, null]"));
        let _ = app.update(Message::NestedEdit(
            vec![PathPart::Index(0)],
            NestedEdit::Remove,
        ));
        assert_eq!(cell(&app, 0, "tags").as_deref(), Some("[b, null]"));

        // scores = {math: 9, art: 7}: rename a key (order kept), add an entry.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 4 }));
        let math = vec![PathPart::Key("math".into())];
        let _ = app.update(Message::EditKey(math, "math".into()));
        let _ = app.update(Message::LeafInput("maths".into()));
        let _ = app.update(Message::CommitLeaf);
        assert!(app.leaf_edit.is_none());
        let _ = app.update(Message::NestedEdit(vec![], NestedEdit::AddEntry));
        assert_eq!(
            cell(&app, 0, "scores").as_deref(),
            Some("{maths: 9, art: 7, key: null}")
        );

        // The JSON editor replaces the whole value.
        let _ = app.update(Message::OpenJsonEditor);
        assert!(matches!(app.dialog, Some(Dialog::Json { .. })));
        use iced::widget::text_editor::{Action as EditorAction, Edit};
        let _ = app.update(Message::Json(dialogs::JsonMessage::Edit(
            EditorAction::SelectAll,
        )));
        let _ = app.update(Message::Json(dialogs::JsonMessage::Edit(
            EditorAction::Edit(Edit::Paste(std::sync::Arc::new("{\"x\": 1".into()))),
        )));
        let _ = app.update(Message::Json(dialogs::JsonMessage::Apply));
        assert!(app.dialog.is_some(), "invalid JSON keeps the editor open");
        let _ = app.update(Message::Json(dialogs::JsonMessage::Edit(
            EditorAction::Edit(Edit::Insert('}')),
        )));
        let _ = app.update(Message::Json(dialogs::JsonMessage::Apply));
        assert!(app.dialog.is_none());
        assert_eq!(cell(&app, 0, "scores").as_deref(), Some("{x: 1}"));
    }

    #[test]
    fn cut_and_paste() {
        let dir = TempDir::new();
        let path = dir.join("m.parquet");
        fixtures::mixed_settings(&path);
        let mut app = app_with(&[&path]);
        let id = app.tabs[0].id;
        let cell = |app: &App, row: usize, column: &str| {
            let doc = app.workbook.get(id).unwrap();
            let i = doc.schema().index_of(column).unwrap();
            veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][i]
                .clone()
        };
        // A block pasted at a cell, as one undo step.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 1 }));
        let _ = app.update(Message::Pasted(Some("x\t1.5\r\ny\t\r\n".into())));
        assert_eq!(cell(&app, 0, "name").as_deref(), Some("x"));
        assert_eq!(cell(&app, 0, "score").as_deref(), Some("1.5"));
        assert_eq!(cell(&app, 1, "name").as_deref(), Some("y"));
        assert_eq!(cell(&app, 1, "score"), None);
        let doc = app.workbook.get(id).unwrap();
        assert_eq!(doc.history().undo_label(), Some("Paste 4 cells"));

        // Cut clears the cell.
        let _ = app.update(Message::Action(Action::Cut));
        assert_eq!(cell(&app, 0, "name"), None);

        // Whole columns: a copied header row is skipped.
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column: 1,
                extend: false,
            },
        ));
        let _ = app.update(Message::Pasted(Some("name\nfirst\n".into())));
        assert_eq!(cell(&app, 0, "name").as_deref(), Some("first"));

        // A bad value reports an error and changes nothing.
        let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
        let before = app.errors.len();
        let _ = app.update(Message::Pasted(Some("abc".into())));
        assert_eq!(app.errors.len(), before + 1);
        assert_eq!(cell(&app, 0, "score").as_deref(), Some("1.5"));
    }

    #[test]
    fn menu_sections_follow_the_target() {
        use veta_core::arrow::datatypes::{DataType, Field, Fields};
        let titles = |target, column_type: Option<&DataType>| -> Vec<&str> {
            panes::menu_sections(target, 1, 1, column_type)
                .into_iter()
                .map(|(title, _)| title)
                .collect()
        };
        let point = DataType::Struct(Fields::from(vec![Field::new("x", DataType::Int64, true)]));
        assert_eq!(titles(MenuTarget::Blank, None), ["Add"]);
        assert_eq!(titles(MenuTarget::Rows, None), ["Clipboard", "Rows"]);
        assert_eq!(
            titles(MenuTarget::Cell, Some(&DataType::Int64)),
            ["Clipboard", "Cell", "Filter", "Rows"]
        );
        assert_eq!(
            titles(MenuTarget::Column(0), Some(&point)),
            ["Clipboard", "Column", "Data", "Split & Merge", "Nested"]
        );
    }
}
