//! [`Message`]: everything the app reacts to.

use super::*;

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
    ColumnDialog(ColumnMessage),
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
