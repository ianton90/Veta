//! Icons from the bundled Lucide subset (`assets/lucide-subset.ttf`).
//!
//! To add an icon, add its codepoint here and regenerate the subset with
//! `pyftsubset` from the full Lucide font, including every codepoint below.

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
    Hash,
    Eraser,
    Scissors,
    InsertRows,
    Swap,
}

impl Icon {
    pub fn codepoint(self) -> char {
        let code = match self {
            Icon::FolderOpen => 0xe247,
            Icon::Save => 0xe151,
            Icon::SaveAll => 0xe414,
            Icon::Close => 0xe1b2,
            Icon::Undo => 0xe2a1,
            Icon::Redo => 0xe2a0,
            Icon::Columns => 0xe09d,
            Icon::Trash => 0xe18e,
            Icon::Filter => 0xe0e0,
            Icon::FilterPlus => 0xe63e,
            Icon::Rows => 0xe58f,
            Icon::SortAsc => 0xe41f,
            Icon::SortDesc => 0xe41a,
            Icon::Type => 0xe198,
            Icon::Replace => 0xe3df,
            Icon::Split => 0xe445,
            Icon::Merge => 0xe444,
            Icon::Rename => 0xe265,
            Icon::CaseUpper => 0xe3de,
            Icon::Sigma => 0xe201,
            Icon::Calculator => 0xe1bc,
            Icon::Calendar => 0xe067,
            Icon::Function => 0xe22d,
            Icon::ListOrdered => 0xe1d1,
            Icon::Copy => 0xe0a2,
            Icon::PanelRight => 0xe436,
            Icon::ListTree => 0xe40d,
            Icon::ChartColumn => 0xe2a3,
            Icon::Sun => 0xe17c,
            Icon::Moon => 0xe122,
            Icon::Monitor => 0xe121,
            Icon::Settings => 0xe158,
            Icon::FileDown => 0xe31b,
            Icon::FileUp => 0xe32b,
            Icon::FilePlus => 0xe0cd,
            Icon::FillDown => 0xe45a,
            Icon::Plus => 0xe141,
            Icon::TableProperties => 0xe4e0,
            Icon::Sliders => 0xe29a,
            Icon::Help => 0xe082,
            Icon::Info => 0xe0ff,
            Icon::ChevronUp => 0xe074,
            Icon::ChevronDown => 0xe071,
            Icon::Hash => 0xe0f3,
            Icon::Eraser => 0xe28f,
            Icon::Scissors => 0xe152,
            Icon::InsertRows => 0xe597,
            Icon::Swap => 0xe41c,
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
