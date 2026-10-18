//! The app state: [`App`] and the types it holds (tabs, dialogs, edits,
//! background work).

use super::*;
use veta_core::display::PathPart;

/// A background import or export, shown in the status bar.
#[derive(Debug)]
pub struct Transfer {
    pub id: u64,
    /// The file being imported, if this is an import.
    pub(crate) importing: Option<PathBuf>,
    pub label: String,
    pub progress: f32,
    pub(crate) cancel: Arc<AtomicBool>,
}

/// Theme colors and base text size, passed to view functions.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ui {
    pub(crate) tokens: Tokens,
    pub(crate) size: f32,
}

impl Ui {
    pub(crate) fn small(self) -> f32 {
        (self.size - 1.0).max(8.0)
    }

    pub(crate) fn heading(self) -> f32 {
        self.size + 2.0
    }
}

#[derive(Debug)]
pub(crate) struct App {
    pub(crate) workbook: Workbook,
    /// Files from other launches, opened once no save is running.
    pub(crate) queued_files: Vec<PathBuf>,
    /// Size of the window, to keep menus on screen.
    pub(crate) window_size: Size,
    /// GUI state per open document, in tab order.
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: Option<DocumentId>,
    pub(crate) show_side_pane: bool,
    pub(crate) side_tab: SideTab,
    /// Files currently being opened in the background.
    pub(crate) opening: Vec<PathBuf>,
    pub(crate) errors: Vec<String>,
    pub(crate) ribbon: Ribbon,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) config: Config,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) themes: Vec<VetaTheme>,
    pub(crate) themes_dir: Option<PathBuf>,
    /// OS light/dark mode, once known.
    pub(crate) system_mode: Option<Mode>,
    pub(crate) text_size: f32,
    /// Cell being edited in the formula bar.
    pub(crate) edit: Option<Edit>,
    pub(crate) menu: Option<ContextMenu>,
    /// Document being saved; the window is blocked meanwhile.
    pub(crate) saving: Option<DocumentId>,
    /// Tabs being closed, waiting for "save changes?" answers.
    pub(crate) closing: Option<Closing>,
    /// The full pass running in the background, if any.
    pub(crate) computing: Option<Computing>,
    /// Column statistics for the side pane.
    pub(crate) stats: Stats,
    /// Exports running in the background.
    /// Imports and exports running in the background.
    pub(crate) transfers: Vec<Transfer>,
    pub(crate) next_transfer: u64,
    /// Last thing done worth telling (shown in the status bar).
    pub(crate) notice: Option<String>,
    /// A "remove the error column if empty" step just added after a cast,
    /// to drop again (keeping the errors) if the column isn't empty.
    pub(crate) error_check: Option<(DocumentId, usize)>,
}

/// A step's full pass running on a worker thread.
#[derive(Debug)]
pub(crate) struct Computing {
    pub(crate) document: DocumentId,
    pub(crate) target: Target,
    /// The job's key; it is stale once the document's key differs.
    pub(crate) key: Vec<u64>,
    pub(crate) step: usize,
    pub(crate) progress: f32,
    pub(crate) cancel: Arc<AtomicBool>,
}

/// Statistics computed for the side pane, and the pass running.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// Results by document, data revision and column.
    pub(crate) cache: HashMap<(DocumentId, u64, String), ColumnStats>,
    pub(crate) running: Option<StatsRun>,
}

#[derive(Debug)]
pub(crate) struct StatsRun {
    pub(crate) document: DocumentId,
    pub(crate) revision: u64,
    pub(crate) columns: Vec<String>,
    pub(crate) progress: f32,
    pub(crate) cancel: Arc<AtomicBool>,
}

/// What a background pass computes for a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// A step of the document.
    Document,
    /// The tab's temporary filter.
    View,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Closing {
    /// Documents still to close, in order. The first one is being asked
    /// about when it has unsaved changes.
    pub(crate) queue: Vec<DocumentId>,
    /// Quit the app once all are closed.
    pub(crate) exit: bool,
}

/// Answer to "save changes before closing?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseAnswer {
    Save,
    Discard,
    Cancel,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Edit {
    pub(crate) document: DocumentId,
    pub(crate) row: usize,
    pub(crate) column: String,
    /// The struct field edited, when the cell is a field of an expanded
    /// struct column.
    pub(crate) field: Option<StructField>,
    /// The line edited, when a line of an open cell is.
    pub(crate) item: Option<CellItem>,
    /// The map entry's key is edited (`item` is the entry, or the slot for
    /// a new one), not its value.
    pub(crate) key: bool,
    pub(crate) text: String,
    /// Why the last attempt to apply it failed.
    pub(crate) error: Option<String>,
}

/// A field inside a struct cell: the field names down to it and its type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StructField {
    pub(crate) path: Vec<String>,
    pub(crate) data_type: veta_core::arrow::datatypes::DataType,
}

/// The selected line of an open cell (a struct field, list item or map
/// entry at any depth, or the slot for a new item), with its cell.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CellItem {
    pub(crate) row: usize,
    pub(crate) column: String,
    /// Struct fields from the column down to the cell, for a field of an
    /// expanded struct column (empty: the column itself).
    pub(crate) cell: Vec<PathPart>,
    pub(crate) item: crate::grid::SelectedItem,
}

impl CellItem {
    /// From the column down to the value (the new item, for a slot).
    pub(crate) fn path(&self) -> Vec<PathPart> {
        [self.cell.clone(), self.item.path.clone()].concat()
    }

    /// From the column down to the struct, list or map holding it.
    pub(crate) fn parent(&self) -> Vec<PathPart> {
        [self.cell.clone(), self.item.parent.clone()].concat()
    }
}

/// Inserting or removing a list item or map entry (see
/// `App::insert_or_remove_item`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemChange {
    InsertAbove,
    InsertBelow,
    Remove,
}

/// What the side pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SideTab {
    #[default]
    Steps,
    Stats,
    File,
}

#[derive(Debug)]
pub(crate) struct Tab {
    pub(crate) id: DocumentId,
    pub(crate) grid: GridView,
    /// Edit-cells steps whose edits are listed in the steps pane, by index.
    pub(crate) expanded_steps: std::collections::BTreeSet<usize>,
}

#[derive(Debug)]
pub(crate) enum Dialog {
    /// A dialog that adds a step, or edits one.
    Step {
        document: DocumentId,
        dialog: Box<dyn StepDialog>,
        /// Editing the step at this index instead of adding a new one.
        replace: Option<usize>,
    },
    Settings(Draft),
    About,
    Column {
        document: DocumentId,
        dialog: ColumnDialog,
        /// Editing the step at this index instead of adding a new one.
        replace: Option<usize>,
    },
    Rows {
        document: DocumentId,
        dialog: RowsDialog,
    },
    Import(ImportDialog),
    Export {
        document: DocumentId,
        dialog: ExportDialog,
    },
    /// "Later steps refer to rows by position" before changing a step.
    ConfirmStep {
        document: DocumentId,
        command: Command,
    },
    /// Replace the document's steps with a recipe's, or add them after.
    ConfirmRecipe {
        document: DocumentId,
        /// The recipe's JSON.
        text: String,
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
