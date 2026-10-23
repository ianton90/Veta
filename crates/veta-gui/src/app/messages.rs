//! [`Message`]: everything the app reacts to.

use super::*;

/// The part of the app that handles a message or a ribbon action: its
/// file (`app/<area>.rs`), with `App` for what `app/mod.rs` handles itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Area {
    App,
    Clipboard,
    Columns,
    Documents,
    Editing,
    FileSettings,
    Filter,
    Menus,
    Recipes,
    Rows,
    Stats,
    Steps,
    Transfers,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Action(Action),
    Ribbon(RibbonMessage),
    FilesPicked(Vec<PathBuf>),
    /// Files sent by another `veta` launch.
    Remote(Vec<PathBuf>),
    WindowResized(Size),
    /// A frame is drawn (only while something animates).
    Frame(std::time::Instant),
    Opened(PathBuf, Result<Loaded, veta_core::Error>),
    /// A CSV import finished, with what it found.
    Imported(
        PathBuf,
        Result<Slot<(Document, ImportReport)>, veta_core::Error>,
    ),
    /// Save As dialog closed (with the chosen path, if any).
    SaveTo(DocumentId, Option<PathBuf>),
    Saved(
        DocumentId,
        Result<Slot<veta_core::SaveResult>, veta_core::Error>,
    ),
    SelectTab(DocumentId),
    CloseTab(DocumentId),
    Grid(DocumentId, GridEvent),
    DismissError(usize),
    Settings(SettingsMessage),
    /// The language changed in Settings: read its texts and redraw.
    LanguageChanged(&'static str),
    CloseDialog,
    CloseRequested,
    CloseAnswer(CloseAnswer),
    /// Esc: closes a dialog or cancels an edit.
    Escape,
    EditInput(String),
    CommitEdit,
    Menu(MenuItem),
    CloseMenu,
    /// For the open step dialog (see [`StepDialog`]).
    Step(StepMessage),
    Rows(RowsMessage),
    /// Progress (0–1) of the background pass with this key.
    ComputeProgress(DocumentId, Target, Vec<u64>, f32),
    /// The background pass with this key finished.
    ComputeDone(
        DocumentId,
        Target,
        Vec<u64>,
        Result<Slot<Computed>, veta_core::Error>,
    ),
    /// Stop the background pass and undo the change that started it.
    CancelCompute,
    SideTab(SideTab),
    /// Show the data after this many steps (past the last: all steps).
    PreviewStep(DocumentId, usize),
    /// Show or hide the edits of an edit-cells step in the steps pane.
    ToggleStep(DocumentId, usize),
    RemoveStep(usize),
    MoveStep(usize, usize),
    EditStep(usize),
    ConfirmStep(bool),
    /// Answer to [`Dialog::ConfirmRecipe`]: replace the steps (`Some(true)`),
    /// add after them (`Some(false)`) or cancel.
    RecipeChoice(Option<bool>),
    /// Save the active document's steps as a recipe.
    StatsProgress(DocumentId, u64, f32),
    /// Clipboard text read for pasting.
    Pasted(Option<String>),
    /// Rows read in the background for a grid (see `GridView::take_read`).
    RowsRead(
        DocumentId,
        u64,
        Result<veta_core::arrow::record_batch::RecordBatch, veta_core::Error>,
    ),
    StatsDone(DocumentId, u64, Result<Vec<ColumnStats>, veta_core::Error>),
    /// A file to import was chosen.
    ImportPicked(Option<PathBuf>),
    Import(ImportMessage),
    Export(ExportMessage),
    /// Where to export was chosen.
    ExportTo(DocumentId, Option<PathBuf>),
    Exported(Result<PathBuf, veta_core::Error>),
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
pub(crate) struct Slot<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Slot<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    pub(crate) fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

pub(crate) type Loaded = Slot<Document>;

impl Message {
    /// Who handles this message (see [`App::handle`]).
    pub(crate) fn area(&self) -> Area {
        use Message as M;
        match self {
            M::Pasted(..) => Area::Clipboard,
            M::Choose(..) => Area::Columns,
            M::EditInput(..) | M::CommitEdit => Area::Editing,
            M::OpenDialog
            | M::FilesPicked(..)
            | M::Remote(..)
            | M::Opened(..)
            | M::SaveTo(..)
            | M::Saved(..)
            | M::SelectTab(..)
            | M::CloseTab(..)
            | M::CloseRequested
            | M::CloseAnswer(..) => Area::Documents,
            M::Imported(..)
            | M::ImportPicked(..)
            | M::Import(..)
            | M::Export(..)
            | M::ExportTo(..)
            | M::Exported(..)
            | M::TransferProgress(..)
            | M::TransferDone(..)
            | M::CancelTransfer(..) => Area::Transfers,
            M::SaveRecipe
            | M::SaveRecipeTo(..)
            | M::LoadRecipe
            | M::LoadRecipeFrom(..)
            | M::RecipeChoice(..) => Area::Recipes,
            M::Writer(..) | M::Metadata(..) => Area::FileSettings,
            M::ClearFilter => Area::Filter,
            M::Menu(..) | M::CloseMenu => Area::Menus,
            M::Rows(..) => Area::Rows,
            M::StatsProgress(..) | M::StatsDone(..) => Area::Stats,
            M::Step(..)
            | M::ComputeProgress(..)
            | M::ComputeDone(..)
            | M::CancelCompute
            | M::PreviewStep(..)
            | M::ToggleStep(..)
            | M::RemoveStep(..)
            | M::MoveStep(..)
            | M::EditStep(..)
            | M::ConfirmStep(..) => Area::Steps,
            M::Action(_)
            | M::Ribbon(_)
            | M::WindowResized(_)
            | M::Frame(_)
            | M::Grid(..)
            | M::DismissError(_)
            | M::Settings(_)
            | M::LanguageChanged(_)
            | M::CloseDialog
            | M::Escape
            | M::RowsRead(..)
            | M::SideTab(_)
            | M::SystemMode(_) => Area::App,
        }
    }
}
