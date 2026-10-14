//! Icons from the bundled Lucide subset (`assets/lucide-subset.ttf`).
//!
//! To add an icon, add it below with its Lucide name and codepoint (from the
//! `lucide-static` package's `font/codepoints.json`; currently 1.56.0), then
//! regenerate the subset with `scripts/icons.sh <path to lucide.ttf>`.

use iced::widget::{Text, text};
use iced::{Font, Pixels};

pub const FONT_BYTES: &[u8] = include_bytes!("../assets/lucide-subset.ttf");
pub const FONT: Font = Font::with_name("lucide");

// Some icons are bundled for commands that are not wired up yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    FolderOpen,
    Save,
    SaveAll,
    Close,
    Undo,
    Redo,
    Columns,
    Trash,
    Filter,
    FilterPlus,
    Rows,
    SortAsc,
    SortDesc,
    Type,
    Replace,
    Split,
    Merge,
    Rename,
    CaseUpper,
    Sigma,
    Calculator,
    Calendar,
    Function,
    ListOrdered,
    Copy,
    PanelRight,
    ListTree,
    ChartColumn,
    Sun,
    Moon,
    Monitor,
    Settings,
    FileDown,
    FileUp,
    FilePlus,
    FillDown,
    Plus,
    TableProperties,
    Sliders,
    Help,
    Info,
    ChevronUp,
    ChevronDown,
    ChevronRight,
    Hash,
    Eraser,
    Scissors,
    InsertRows,
    Swap,
    Paste,
    ArrowLeft,
    ArrowRight,
    Braces,
    Layers,
    FillUp,
    RecipeLoad,
    RecipeSave,
}

impl Icon {
    pub fn codepoint(self) -> char {
        let code = match self {
            Icon::FolderOpen => 0xe247,      // folder-open
            Icon::Save => 0xe14d,            // save
            Icon::SaveAll => 0xe40f,         // save-all
            Icon::Close => 0xe1b2,           // x
            Icon::Undo => 0xe2a1,            // undo-2
            Icon::Redo => 0xe2a0,            // redo-2
            Icon::Columns => 0xe099,         // columns-3
            Icon::Trash => 0xe18e,           // trash-2
            Icon::Filter => 0xe0dc,          // funnel
            Icon::FilterPlus => 0xe0dd,      // funnel-plus
            Icon::Rows => 0xe58a,            // rows-3
            Icon::SortAsc => 0xe41a,         // arrow-up-a-z
            Icon::SortDesc => 0xe415,        // arrow-down-a-z
            Icon::Type => 0xe198,            // type
            Icon::Replace => 0xe3db,         // replace
            Icon::Split => 0xe440,           // split
            Icon::Merge => 0xe43f,           // merge
            Icon::Rename => 0xe265,          // text-cursor-input
            Icon::CaseUpper => 0xe3da,       // case-upper
            Icon::Sigma => 0xe201,           // sigma
            Icon::Calculator => 0xe1bc,      // calculator
            Icon::Calendar => 0xe063,        // calendar
            Icon::Function => 0xe22d,        // square-function
            Icon::ListOrdered => 0xe1d1,     // list-ordered
            Icon::Copy => 0xe09e,            // copy
            Icon::PanelRight => 0xe431,      // panel-right
            Icon::ListTree => 0xe408,        // list-tree
            Icon::ChartColumn => 0xe2a3,     // chart-column
            Icon::Sun => 0xe178,             // sun
            Icon::Moon => 0xe11e,            // moon
            Icon::Monitor => 0xe11d,         // monitor
            Icon::Settings => 0xe154,        // settings
            Icon::FileDown => 0xe318,        // file-down
            Icon::FileUp => 0xe32a,          // file-up
            Icon::FilePlus => 0xe0c9,        // file-plus
            Icon::FillDown => 0xe455,        // arrow-down-to-line
            Icon::Plus => 0xe13d,            // plus
            Icon::TableProperties => 0xe4db, // table-properties
            Icon::Sliders => 0xe29a,         // sliders-horizontal
            Icon::Help => 0xe082,            // circle-help
            Icon::Info => 0xe0f9,            // info
            Icon::ChevronUp => 0xe070,       // chevron-up
            Icon::ChevronDown => 0xe06d,     // chevron-down
            Icon::ChevronRight => 0xe06f,    // chevron-right
            Icon::Hash => 0xe0ef,            // hash
            Icon::Eraser => 0xe28f,          // eraser
            Icon::Scissors => 0xe14e,        // scissors
            Icon::InsertRows => 0xe592,      // between-horizontal-start
            Icon::Swap => 0xe417,            // arrow-right-left
            Icon::Paste => 0xe3e8,           // clipboard-paste
            Icon::ArrowLeft => 0xe048,       // arrow-left
            Icon::ArrowRight => 0xe049,      // arrow-right
            Icon::Braces => 0xe36a,          // braces
            Icon::Layers => 0xe529,          // layers
            Icon::FillUp => 0xe45b,          // arrow-up-to-line
            Icon::RecipeLoad => 0xe0c5,      // file-input
            Icon::RecipeSave => 0xe0c8,      // file-output
        };
        char::from_u32(code).unwrap_or('?')
    }
}

/// An icon as a text widget.
pub fn icon<'a>(icon: Icon, size: impl Into<Pixels>) -> Text<'a> {
    text(icon.codepoint())
        .font(FONT)
        .size(size)
        .line_height(1.0)
}
