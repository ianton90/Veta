//! Dialogs for column commands (add, rename, choose columns, change type,
//! replace values, format text), filters, sorts and writer settings.

use iced::widget::{Space, button, checkbox, column, pick_list, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use veta_core::arrow::array::ArrayRef;
use veta_core::arrow::datatypes::{DataType, TimeUnit};
use veta_core::io::ARROW_SCHEMA_KEY;
use veta_core::model::KeyValue;
use veta_core::steps::{
    Cast, CastOptions, CastPreview, Condition, FORMAT_CODES, Filter, FilterOp, OnError, Replace,
    Sort, SortKey, TextOp,
};
use veta_core::steps::{Merge, Split, SplitBy, count_parts};
use veta_core::values::is_text;
use veta_core::{
    ColumnSettings, Command, Compression, Encoding, FormatVersion, StatisticsLevel, WriterSettings,
};

use crate::settings::card;
use crate::theme::Tokens;

/// Id of the name input, focused when a column dialog opens.
pub const NAME_ID: iced::widget::Id = iced::widget::Id::new("column-name");

/// Types offered when adding a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeChoice {
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
    Timestamp,
}

impl TypeChoice {
    const ALL: [TypeChoice; 6] = [
        Self::Text,
        Self::Integer,
        Self::Decimal,
        Self::Boolean,
        Self::Date,
        Self::Timestamp,
    ];

    /// The choice for `data_type`, if it is one offered.
    pub fn from_type(data_type: &DataType) -> Option<Self> {
        Self::ALL.into_iter().find(|c| &c.data_type() == data_type)
    }

    fn data_type(self) -> DataType {
        match self {
            Self::Text => DataType::Utf8,
            Self::Integer => DataType::Int64,
            Self::Decimal => DataType::Float64,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
        }
    }
}

impl std::fmt::Display for TypeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text => "Text (string)",
            Self::Integer => "Whole number (int64)",
            Self::Decimal => "Decimal number (float64)",
            Self::Boolean => "True/false (bool)",
            Self::Date => "Date",
            Self::Timestamp => "Date and time (timestamp)",
        })
    }
}

/// Add or rename a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDialog {
    pub mode: ColumnMode,
    name: String,
    choice: TypeChoice,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnMode {
    Add { at: usize },
    Rename { from: String },
}

#[derive(Debug, Clone)]
pub enum ColumnMessage {
    Name(String),
    Type(TypeChoice),
    Submit,
    Cancel,
}

impl ColumnDialog {
    pub fn add(at: usize) -> Self {
        Self {
            mode: ColumnMode::Add { at },
            name: String::new(),
            choice: TypeChoice::Text,
        }
    }

    /// Starts with `name` in the name field.
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    /// Starts with the choice matching `data_type`, if it is one offered.
    pub fn with_type(mut self, data_type: &DataType) -> Self {
        if let Some(choice) = TypeChoice::from_type(data_type) {
            self.choice = choice;
        }
        self
    }

    pub fn rename(from: String) -> Self {
        Self {
            name: from.clone(),
            mode: ColumnMode::Rename { from },
            choice: TypeChoice::Text,
        }
    }

    /// Handles a message. Returns the command to run on submit.
    pub fn update(&mut self, message: ColumnMessage) -> Option<Command> {
        match message {
            ColumnMessage::Name(name) => self.name = name,
            ColumnMessage::Type(choice) => self.choice = choice,
            ColumnMessage::Cancel => {}
            ColumnMessage::Submit => {
                let name = self.name.trim().to_owned();
                return Some(match &self.mode {
                    ColumnMode::Add { at } => Command::AddColumn {
                        name,
                        data_type: self.choice.data_type(),
                        at: *at,
                    },
                    ColumnMode::Rename { from } => Command::RenameColumn {
                        from: from.clone(),
                        to: name,
                    },
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ColumnMessage> {
        let (title, action) = match &self.mode {
            ColumnMode::Add { .. } => ("Add column", "Add"),
            ColumnMode::Rename { .. } => ("Rename column", "Rename"),
        };
        let mut body = column![
            text(title).size(20),
            row![
                text("Name").width(70),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(ColumnMessage::Name)
                    .on_submit(ColumnMessage::Submit),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(16);
        if let ColumnMode::Add { .. } = self.mode {
            body = body.push(
                row![
                    text("Type").width(70),
                    pick_list(TypeChoice::ALL, Some(self.choice), ColumnMessage::Type)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
            body = body.push(text("The new column starts empty (null).").color(tokens.muted_text));
        }
        body = body.push(buttons(
            action,
            ColumnMessage::Cancel,
            ColumnMessage::Submit,
        ));
        card(body.into(), tokens, 420.0)
    }
}

/// Pick the columns to keep (Power Query's "Choose Columns").
#[derive(Debug, Clone, PartialEq)]
pub struct ChooseColumns {
    columns: Vec<(String, bool)>,
}

#[derive(Debug, Clone)]
pub enum ChooseMessage {
    Toggle(usize, bool),
    All(bool),
    Apply,
    Cancel,
}

impl ChooseColumns {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            columns: names.into_iter().map(|n| (n, true)).collect(),
        }
    }

    /// Handles a message. Returns the command to run on apply, if any
    /// column was unchecked.
    pub fn update(&mut self, message: ChooseMessage) -> Option<Option<Command>> {
        match message {
            ChooseMessage::Toggle(i, keep) => {
                if let Some(column) = self.columns.get_mut(i) {
                    column.1 = keep;
                }
            }
            ChooseMessage::All(keep) => {
                for column in &mut self.columns {
                    column.1 = keep;
                }
            }
            ChooseMessage::Cancel => {}
            ChooseMessage::Apply => {
                let names: Vec<String> = self
                    .columns
                    .iter()
                    .filter(|(_, keep)| !keep)
                    .map(|(name, _)| name.clone())
                    .collect();
                return Some((!names.is_empty()).then_some(Command::RemoveColumns { names }));
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ChooseMessage> {
        let all = self.columns.iter().all(|(_, keep)| *keep);
        let list = column(self.columns.iter().enumerate().map(|(i, (name, keep))| {
            checkbox(*keep)
                .label(name.as_str())
                .on_toggle(move |keep| ChooseMessage::Toggle(i, keep))
                .into()
        }))
        .spacing(6);
        let kept = self.columns.iter().filter(|(_, keep)| *keep).count();
        let body = column![
            text("Choose columns").size(20),
            checkbox(all)
                .label("Select all")
                .on_toggle(ChooseMessage::All),
            scrollable(list).height(Length::Fixed(320.0)),
            text(format!("{kept} of {} columns kept", self.columns.len())).color(tokens.muted_text),
            buttons("OK", ChooseMessage::Cancel, ChooseMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 420.0)
    }
}

/// Keep rows that match conditions (Power Query's "Filter Rows").
#[derive(Debug, Clone, PartialEq)]
pub struct FilterDialog {
    columns: Vec<String>,
    conditions: Vec<Condition>,
    matching: Matching,
    error: Option<String>,
}

/// Whether every condition or any of them must hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matching {
    All,
    Any,
}

impl std::fmt::Display for Matching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::All => "all conditions",
            Self::Any => "any condition",
        })
    }
}

#[derive(Debug, Clone)]
pub enum FilterMessage {
    Column(usize, String),
    Op(usize, FilterOp),
    Value(usize, String),
    CaseSensitive(usize, bool),
    Add,
    Remove(usize),
    Matching(Matching),
    Apply,
    Cancel,
}

impl FilterDialog {
    /// A dialog with one condition on `column` (or the first column).
    pub fn new(columns: Vec<String>, column: Option<String>) -> Self {
        let column = column
            .or_else(|| columns.first().cloned())
            .unwrap_or_default();
        Self {
            columns,
            conditions: vec![Condition {
                column,
                op: FilterOp::Equals,
                value: String::new(),
                case_sensitive: false,
            }],
            matching: Matching::All,
            error: None,
        }
    }

    /// A dialog editing an existing filter.
    pub fn edit(columns: Vec<String>, filter: &Filter) -> Self {
        Self {
            columns,
            conditions: filter.conditions.clone(),
            matching: if filter.match_all {
                Matching::All
            } else {
                Matching::Any
            },
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Handles a message. Returns the filter to apply on submit.
    pub fn update(&mut self, message: FilterMessage) -> Option<Filter> {
        self.error = None;
        match message {
            FilterMessage::Column(i, column) => self.edit_condition(i, |c| c.column = column),
            FilterMessage::Op(i, op) => self.edit_condition(i, |c| c.op = op),
            FilterMessage::Value(i, value) => self.edit_condition(i, |c| c.value = value),
            FilterMessage::CaseSensitive(i, on) => {
                self.edit_condition(i, |c| c.case_sensitive = on)
            }
            FilterMessage::Add => {
                let column = self.conditions.last().map(|c| c.column.clone());
                let first = Self::new(self.columns.clone(), column).conditions;
                self.conditions.extend(first);
            }
            FilterMessage::Remove(i) => {
                if self.conditions.len() > 1 && i < self.conditions.len() {
                    self.conditions.remove(i);
                }
            }
            FilterMessage::Matching(matching) => self.matching = matching,
            FilterMessage::Cancel => {}
            FilterMessage::Apply => {
                return Some(Filter {
                    conditions: self.conditions.clone(),
                    match_all: self.matching == Matching::All,
                });
            }
        }
        None
    }

    fn edit_condition(&mut self, i: usize, change: impl FnOnce(&mut Condition)) {
        if let Some(condition) = self.conditions.get_mut(i) {
            change(condition);
        }
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, FilterMessage> {
        let mut list = column![].spacing(8);
        for (i, c) in self.conditions.iter().enumerate() {
            let mut value = text_input("Value", &c.value).width(Length::Fill);
            if c.op.needs_value() {
                value = value
                    .on_input(move |v| FilterMessage::Value(i, v))
                    .on_submit(FilterMessage::Apply);
            }
            let mut remove = button(text("✕")).style(button::text);
            if self.conditions.len() > 1 {
                remove = remove.on_press(FilterMessage::Remove(i));
            }
            list = list.push(
                row![
                    pick_list(self.columns.clone(), Some(c.column.clone()), move |col| {
                        FilterMessage::Column(i, col)
                    })
                    .width(160),
                    pick_list(FilterOp::ALL, Some(c.op), move |op| FilterMessage::Op(
                        i, op
                    ))
                    .width(190),
                    value,
                    checkbox(c.case_sensitive)
                        .label("Aa")
                        .on_toggle(move |on| FilterMessage::CaseSensitive(i, on)),
                    remove,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let mut body = column![
            text("Filter rows").size(20),
            row![
                text("Keep rows that match"),
                pick_list(
                    [Matching::All, Matching::Any],
                    Some(self.matching),
                    FilterMessage::Matching
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            scrollable(list).height(Length::Shrink),
            button(text("+ Add condition"))
                .style(button::text)
                .on_press(FilterMessage::Add),
            text("Values are read as the column's type. \"Aa\" matches text case-sensitively.")
                .color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", FilterMessage::Cancel, FilterMessage::Apply));
        card(body.into(), tokens, 720.0)
    }
}

/// A separator choice for numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    None,
    Dot,
    Comma,
    Space,
    Apostrophe,
}

impl Separator {
    fn char(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::Dot => Some('.'),
            Self::Comma => Some(','),
            Self::Space => Some(' '),
            Self::Apostrophe => Some('\''),
        }
    }

    fn from_char(c: Option<char>) -> Self {
        match c {
            Some('.') => Self::Dot,
            Some(',') => Self::Comma,
            Some(' ') => Self::Space,
            Some('\'') => Self::Apostrophe,
            _ => Self::None,
        }
    }
}

impl std::fmt::Display for Separator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::Dot => "dot (1.5)",
            Self::Comma => "comma (1,5)",
            Self::Space => "space",
            Self::Apostrophe => "apostrophe",
        })
    }
}

/// Change a column's type (Power Query's "Change Type … Using Locale").
#[derive(Debug, Clone)]
pub struct CastDialog {
    column: String,
    from_text: bool,
    choice: TypeChoice,
    decimal: Separator,
    thousands: Separator,
    format: String,
    on_error: OnError,
    /// The first rows of the column, to preview failures on.
    sample: ArrayRef,
    preview: CastPreview,
}

#[derive(Debug, Clone)]
pub enum CastMessage {
    Type(TypeChoice),
    Decimal(Separator),
    Thousands(Separator),
    Format(String),
    OnError(OnError),
    Apply,
    Cancel,
}

impl CastDialog {
    /// A dialog for `column`, whose first values are `sample`. `cast` fills
    /// it in when editing a step.
    pub fn new(column: String, sample: ArrayRef, cast: Option<&Cast>) -> Self {
        let from_text = is_text(sample.data_type());
        let options = cast.map(|c| c.options.clone()).unwrap_or_default();
        let choice = cast
            .and_then(|c| TypeChoice::from_type(&c.to))
            .unwrap_or(if from_text {
                TypeChoice::Decimal
            } else {
                TypeChoice::Text
            });
        let mut dialog = Self {
            column,
            from_text,
            choice,
            decimal: Separator::from_char(Some(options.decimal)),
            thousands: Separator::from_char(options.thousands),
            format: options.format.unwrap_or_default(),
            on_error: options.on_error,
            sample,
            preview: CastPreview::default(),
        };
        dialog.refresh();
        dialog
    }

    fn uses_format(&self) -> bool {
        self.from_text && matches!(self.choice, TypeChoice::Date | TypeChoice::Timestamp)
    }

    fn uses_separators(&self) -> bool {
        self.from_text && matches!(self.choice, TypeChoice::Integer | TypeChoice::Decimal)
    }

    fn cast(&self) -> Cast {
        let format = self.format.trim();
        Cast {
            column: self.column.clone(),
            to: self.choice.data_type(),
            options: CastOptions {
                decimal: self.decimal.char().unwrap_or('.'),
                thousands: self.thousands.char(),
                format: (self.uses_format() && !format.is_empty()).then(|| format.to_owned()),
                on_error: self.on_error,
            },
        }
    }

    fn refresh(&mut self) {
        self.preview = self.cast().preview(&self.sample);
    }

    /// Handles a message. Returns the cast to apply on submit.
    pub fn update(&mut self, message: CastMessage) -> Option<Cast> {
        match message {
            CastMessage::Type(choice) => self.choice = choice,
            CastMessage::Decimal(s) => self.decimal = s,
            CastMessage::Thousands(s) => self.thousands = s,
            CastMessage::Format(f) => self.format = f,
            CastMessage::OnError(e) => self.on_error = e,
            CastMessage::Cancel => return None,
            CastMessage::Apply => return Some(self.cast()),
        }
        self.refresh();
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, CastMessage> {
        const LABEL: f32 = 130.0;
        let labelled = |label: &'static str, control: Element<'static, CastMessage>| {
            row![text(label).width(LABEL), control]
                .spacing(12)
                .align_y(Alignment::Center)
        };
        let mut body = column![
            text(format!("Change type of {}", self.column)).size(20),
            labelled(
                "Type",
                pick_list(TypeChoice::ALL, Some(self.choice), CastMessage::Type)
                    .width(Length::Fill)
                    .into()
            ),
        ]
        .spacing(14);
        if self.uses_separators() {
            body = body.push(labelled(
                "Decimal separator",
                pick_list(
                    [Separator::Dot, Separator::Comma],
                    Some(self.decimal),
                    CastMessage::Decimal,
                )
                .width(Length::Fill)
                .into(),
            ));
            body = body.push(labelled(
                "Thousands",
                pick_list(
                    [
                        Separator::None,
                        Separator::Comma,
                        Separator::Dot,
                        Separator::Space,
                        Separator::Apostrophe,
                    ],
                    Some(self.thousands),
                    CastMessage::Thousands,
                )
                .width(Length::Fill)
                .into(),
            ));
        }
        if self.uses_format() {
            body = body.push(labelled(
                "Format",
                text_input("Automatic (ISO 8601: 2024-12-31)", &self.format)
                    .on_input(CastMessage::Format)
                    .on_submit(CastMessage::Apply)
                    .into(),
            ));
            body = body.push(text(FORMAT_CODES).size(12).color(tokens.muted_text));
        }
        body = body.push(labelled(
            "Invalid values",
            pick_list(OnError::ALL, Some(self.on_error), CastMessage::OnError)
                .width(Length::Fill)
                .into(),
        ));
        let p = &self.preview;
        let summary = if p.checked == 0 {
            "No rows to check.".to_owned()
        } else if p.failed == 0 {
            format!("All of the first {} rows convert.", p.checked)
        } else {
            let examples: Vec<String> = p.examples.iter().map(|e| format!("{e:?}")).collect();
            format!(
                "{} of the first {} rows can't be converted, e.g. {}.",
                p.failed,
                p.checked,
                examples.join(", ")
            )
        };
        let color = if p.failed == 0 {
            tokens.muted_text
        } else {
            iced::Color::from_rgb8(0xd1, 0x43, 0x43)
        };
        body = body.push(text(summary).color(color));
        body = body.push(buttons("OK", CastMessage::Cancel, CastMessage::Apply));
        card(body.into(), tokens, 520.0)
    }
}

/// Replace values in a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplaceDialog {
    column: String,
    text_column: bool,
    find: String,
    find_null: bool,
    with: String,
    with_null: bool,
    whole_cell: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ReplaceMessage {
    Find(String),
    FindNull(bool),
    With(String),
    WithNull(bool),
    WholeCell(bool),
    Apply,
    Cancel,
}

impl ReplaceDialog {
    pub fn new(column: String, text_column: bool, replace: Option<&Replace>) -> Self {
        Self {
            column,
            text_column,
            find: replace.and_then(|r| r.find.clone()).unwrap_or_default(),
            find_null: replace.is_some_and(|r| r.find.is_none()),
            with: replace.and_then(|r| r.with.clone()).unwrap_or_default(),
            with_null: replace.is_some_and(|r| r.with.is_none()),
            whole_cell: replace.is_some_and(|r| r.whole_cell),
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: ReplaceMessage) -> Option<Replace> {
        self.error = None;
        match message {
            ReplaceMessage::Find(t) => self.find = t,
            ReplaceMessage::FindNull(on) => self.find_null = on,
            ReplaceMessage::With(t) => self.with = t,
            ReplaceMessage::WithNull(on) => self.with_null = on,
            ReplaceMessage::WholeCell(on) => self.whole_cell = on,
            ReplaceMessage::Cancel => {}
            ReplaceMessage::Apply => {
                return Some(Replace {
                    column: self.column.clone(),
                    find: (!self.find_null).then(|| self.find.clone()),
                    with: (!self.with_null).then(|| self.with.clone()),
                    whole_cell: self.whole_cell || !self.text_column,
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ReplaceMessage> {
        let mut find = text_input("Value to find", &self.find).id(NAME_ID);
        if !self.find_null {
            find = find
                .on_input(ReplaceMessage::Find)
                .on_submit(ReplaceMessage::Apply);
        }
        let mut with = text_input("Replace with", &self.with);
        if !self.with_null {
            with = with
                .on_input(ReplaceMessage::With)
                .on_submit(ReplaceMessage::Apply);
        }
        let mut body = column![
            text(format!("Replace values in {}", self.column)).size(20),
            row![
                text("Find").width(80),
                find,
                checkbox(self.find_null)
                    .label("null")
                    .on_toggle(ReplaceMessage::FindNull),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text("Replace with").width(80),
                with,
                checkbox(self.with_null)
                    .label("null")
                    .on_toggle(ReplaceMessage::WithNull),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(14);
        if self.text_column && !self.find_null {
            body = body.push(
                checkbox(self.whole_cell)
                    .label("Match entire cell contents")
                    .on_toggle(ReplaceMessage::WholeCell),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        } else if !self.text_column {
            body = body.push(
                text("Values are read as the column's type and match whole cells.")
                    .color(tokens.muted_text),
            );
        }
        body = body.push(buttons("OK", ReplaceMessage::Cancel, ReplaceMessage::Apply));
        card(body.into(), tokens, 480.0)
    }
}

/// Trim or change the case of text columns.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatDialog {
    columns: Vec<String>,
    op: TextOp,
}

#[derive(Debug, Clone)]
pub enum FormatMessage {
    Op(TextOp),
    Apply,
    Cancel,
}

impl FormatDialog {
    pub fn new(columns: Vec<String>, op: TextOp) -> Self {
        Self { columns, op }
    }

    /// Returns the columns and operation on submit.
    pub fn update(&mut self, message: FormatMessage) -> Option<(Vec<String>, TextOp)> {
        match message {
            FormatMessage::Op(op) => self.op = op,
            FormatMessage::Cancel => {}
            FormatMessage::Apply => return Some((self.columns.clone(), self.op)),
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, FormatMessage> {
        let body = column![
            text("Format text").size(20),
            text(self.columns.join(", ")).color(tokens.muted_text),
            pick_list(TextOp::ALL, Some(self.op), FormatMessage::Op).width(Length::Fill),
            buttons("OK", FormatMessage::Cancel, FormatMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 400.0)
    }
}

/// How to split a column, for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitMode {
    Delimiter,
    Positions,
}

impl std::fmt::Display for SplitMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Delimiter => "by delimiter",
            Self::Positions => "at character positions",
        })
    }
}

/// Split a column into several.
#[derive(Debug, Clone)]
pub struct SplitDialog {
    column: String,
    sample: ArrayRef,
    mode: SplitMode,
    delimiter: String,
    parts: String,
    positions: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SplitMessage {
    Mode(SplitMode),
    Delimiter(String),
    Parts(String),
    Positions(String),
    Apply,
    Cancel,
}

impl SplitDialog {
    /// `sample` (first values of the column) suggests how many parts.
    pub fn new(column: String, sample: ArrayRef, split: Option<&Split>) -> Self {
        let mut dialog = Self {
            column,
            sample,
            mode: SplitMode::Delimiter,
            delimiter: ",".into(),
            parts: String::new(),
            positions: String::new(),
            error: None,
        };
        match split.map(|s| &s.by) {
            Some(SplitBy::Delimiter { delimiter, parts }) => {
                dialog.delimiter = delimiter.clone();
                dialog.parts = parts.to_string();
            }
            Some(SplitBy::Positions(positions)) => {
                dialog.mode = SplitMode::Positions;
                let list: Vec<String> = positions.iter().map(ToString::to_string).collect();
                dialog.positions = list.join(", ");
                dialog.suggest_parts();
            }
            None => dialog.suggest_parts(),
        }
        dialog
    }

    fn suggest_parts(&mut self) {
        self.parts = count_parts(&self.sample, &self.delimiter)
            .max(2)
            .to_string();
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: SplitMessage) -> Option<Split> {
        self.error = None;
        match message {
            SplitMessage::Mode(mode) => self.mode = mode,
            SplitMessage::Delimiter(d) => {
                self.delimiter = d;
                self.suggest_parts();
            }
            SplitMessage::Parts(p) => self.parts = p,
            SplitMessage::Positions(p) => self.positions = p,
            SplitMessage::Cancel => {}
            SplitMessage::Apply => {
                let by = match self.mode {
                    SplitMode::Delimiter => match self.parts.trim().parse::<usize>() {
                        Ok(parts) => SplitBy::Delimiter {
                            delimiter: self.delimiter.clone(),
                            parts,
                        },
                        Err(_) => {
                            self.error = Some("Columns must be a whole number.".into());
                            return None;
                        }
                    },
                    SplitMode::Positions => {
                        let parsed: Result<Vec<usize>, _> = self
                            .positions
                            .split(',')
                            .filter(|p| !p.trim().is_empty())
                            .map(|p| p.trim().parse::<usize>())
                            .collect();
                        match parsed {
                            Ok(positions) => SplitBy::Positions(positions),
                            Err(_) => {
                                self.error =
                                    Some("Positions must be whole numbers, like 2, 5.".into());
                                return None;
                            }
                        }
                    }
                };
                return Some(Split {
                    column: self.column.clone(),
                    by,
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, SplitMessage> {
        let labelled = |label: &'static str, control: Element<'static, SplitMessage>| {
            row![text(label).width(90), control]
                .spacing(12)
                .align_y(Alignment::Center)
        };
        let mut body = column![
            text(format!("Split {}", self.column)).size(20),
            labelled(
                "Split",
                pick_list(
                    [SplitMode::Delimiter, SplitMode::Positions],
                    Some(self.mode),
                    SplitMessage::Mode
                )
                .width(Length::Fill)
                .into()
            ),
        ]
        .spacing(14);
        match self.mode {
            SplitMode::Delimiter => {
                body = body.push(labelled(
                    "Delimiter",
                    text_input(",", &self.delimiter)
                        .id(NAME_ID)
                        .on_input(SplitMessage::Delimiter)
                        .on_submit(SplitMessage::Apply)
                        .into(),
                ));
                body = body.push(labelled(
                    "Columns",
                    text_input("2", &self.parts)
                        .on_input(SplitMessage::Parts)
                        .on_submit(SplitMessage::Apply)
                        .width(80)
                        .into(),
                ));
                body = body.push(
                    text("The last column keeps any text left over.").color(tokens.muted_text),
                );
            }
            SplitMode::Positions => {
                body = body.push(labelled(
                    "Positions",
                    text_input("e.g. 2, 5", &self.positions)
                        .id(NAME_ID)
                        .on_input(SplitMessage::Positions)
                        .on_submit(SplitMessage::Apply)
                        .into(),
                ));
                body = body.push(
                    text("Character counts from the start where each new column begins.")
                        .color(tokens.muted_text),
                );
            }
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", SplitMessage::Cancel, SplitMessage::Apply));
        card(body.into(), tokens, 440.0)
    }
}

/// Merge columns into one.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeDialog {
    columns: Vec<String>,
    separator: String,
    name: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MergeMessage {
    Separator(String),
    Name(String),
    Apply,
    Cancel,
}

impl MergeDialog {
    pub fn new(columns: Vec<String>, merge: Option<&Merge>) -> Self {
        Self {
            separator: merge.map_or_else(|| " ".into(), |m| m.separator.clone()),
            name: merge.map_or_else(|| "Merged".into(), |m| m.name.clone()),
            columns,
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: MergeMessage) -> Option<Merge> {
        self.error = None;
        match message {
            MergeMessage::Separator(s) => self.separator = s,
            MergeMessage::Name(n) => self.name = n,
            MergeMessage::Cancel => {}
            MergeMessage::Apply => {
                return Some(Merge {
                    columns: self.columns.clone(),
                    separator: self.separator.clone(),
                    name: self.name.trim().to_owned(),
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, MergeMessage> {
        let mut body = column![
            text("Merge columns").size(20),
            text(self.columns.join(", ")).color(tokens.muted_text),
            row![
                text("Separator").width(90),
                text_input("none", &self.separator)
                    .on_input(MergeMessage::Separator)
                    .on_submit(MergeMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text("New column").width(90),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(MergeMessage::Name)
                    .on_submit(MergeMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            text("Nulls count as empty text.").color(tokens.muted_text),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons("OK", MergeMessage::Cancel, MergeMessage::Apply));
        card(body.into(), tokens, 440.0)
    }
}

/// Sort by one or more columns.
#[derive(Debug, Clone, PartialEq)]
pub struct SortDialog {
    columns: Vec<String>,
    keys: Vec<SortKey>,
}

/// Sort direction, for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        })
    }
}

#[derive(Debug, Clone)]
pub enum SortMessage {
    Column(usize, String),
    Direction(usize, Direction),
    Add,
    Remove(usize),
    Apply,
    Cancel,
}

impl SortDialog {
    pub fn new(columns: Vec<String>, sort: &Sort) -> Self {
        Self {
            columns,
            keys: sort.keys.clone(),
        }
    }

    /// Handles a message. Returns the sort to apply on submit.
    pub fn update(&mut self, message: SortMessage) -> Option<Sort> {
        match message {
            SortMessage::Column(i, column) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.column = column;
                }
            }
            SortMessage::Direction(i, direction) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.descending = direction == Direction::Descending;
                }
            }
            SortMessage::Add => {
                let column = self
                    .columns
                    .iter()
                    .find(|c| !self.keys.iter().any(|k| &k.column == *c))
                    .or(self.columns.first())
                    .cloned()
                    .unwrap_or_default();
                self.keys.push(SortKey {
                    column,
                    descending: false,
                });
            }
            SortMessage::Remove(i) => {
                if self.keys.len() > 1 && i < self.keys.len() {
                    self.keys.remove(i);
                }
            }
            SortMessage::Cancel => {}
            SortMessage::Apply => {
                return Some(Sort {
                    keys: self.keys.clone(),
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, SortMessage> {
        let mut list = column![].spacing(8);
        for (i, key) in self.keys.iter().enumerate() {
            let direction = if key.descending {
                Direction::Descending
            } else {
                Direction::Ascending
            };
            let mut remove = button(text("✕")).style(button::text);
            if self.keys.len() > 1 {
                remove = remove.on_press(SortMessage::Remove(i));
            }
            list = list.push(
                row![
                    text(if i == 0 { "Sort by" } else { "then by" }).width(60),
                    pick_list(self.columns.clone(), Some(key.column.clone()), move |c| {
                        SortMessage::Column(i, c)
                    })
                    .width(Length::Fill),
                    pick_list(
                        [Direction::Ascending, Direction::Descending],
                        Some(direction),
                        move |d| SortMessage::Direction(i, d)
                    )
                    .width(130),
                    remove,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        let body = column![
            text("Sort rows").size(20),
            list,
            button(text("+ Add column"))
                .style(button::text)
                .on_press(SortMessage::Add),
            text("Nulls come last. Rows that tie keep their order.").color(tokens.muted_text),
            buttons("OK", SortMessage::Cancel, SortMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 480.0)
    }
}

/// Edit the settings used when saving: file-wide settings, defaults for new
/// columns, and per-column settings.
#[derive(Debug, Clone, PartialEq)]
pub struct WriterDialog {
    settings: WriterSettings,
    /// Top-level columns and their types, in order.
    columns: Vec<(String, DataType)>,
    row_group: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WriterMessage {
    Version(FormatVersion),
    RowGroup(String),
    /// `None` edits the defaults for new columns.
    Compression(Option<usize>, Compression),
    Encoding(usize, Encoding),
    Dictionary(Option<usize>, bool),
    Statistics(Option<usize>, StatisticsLevel),
    Bloom(Option<usize>, bool),
    Apply,
    Cancel,
}

impl WriterDialog {
    pub fn new(settings: WriterSettings, columns: Vec<(String, DataType)>) -> Self {
        Self {
            row_group: settings.max_row_group_rows.to_string(),
            settings,
            columns,
            error: None,
        }
    }

    /// Handles a message. Returns the command to run on apply.
    pub fn update(&mut self, message: WriterMessage) -> Option<Command> {
        match message {
            WriterMessage::Version(v) => self.settings.format_version = v,
            WriterMessage::RowGroup(text) => self.row_group = text,
            WriterMessage::Compression(i, c) => self.edit(i, |s| s.compression = c),
            WriterMessage::Encoding(i, e) => self.edit(Some(i), |s| s.encoding = e),
            WriterMessage::Dictionary(i, on) => self.edit(i, |s| s.dictionary = on),
            WriterMessage::Statistics(i, level) => self.edit(i, |s| s.statistics = level),
            WriterMessage::Bloom(i, on) => self.edit(i, |s| s.bloom_filter = on),
            WriterMessage::Cancel => {}
            WriterMessage::Apply => match self.row_group.trim().parse::<usize>() {
                Ok(rows) if rows > 0 => {
                    let mut settings = self.settings.clone();
                    settings.max_row_group_rows = rows;
                    return Some(Command::SetWriterSettings(settings));
                }
                _ => self.error = Some("Rows per row group must be a whole number above 0.".into()),
            },
        }
        None
    }

    fn column_settings(&self, i: Option<usize>) -> ColumnSettings {
        match i.and_then(|i| self.columns.get(i)) {
            Some((name, _)) => *self.settings.column(name),
            None => self.settings.default_column,
        }
    }

    fn edit(&mut self, i: Option<usize>, change: impl FnOnce(&mut ColumnSettings)) {
        let Some((name, _)) = i.and_then(|i| self.columns.get(i)) else {
            change(&mut self.settings.default_column);
            return;
        };
        let mut column = *self.settings.column(name);
        change(&mut column);
        match self.settings.columns.iter_mut().find(|(p, _)| p == name) {
            Some((_, existing)) => *existing = column,
            None => self.settings.columns.push((name.clone(), column)),
        }
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, WriterMessage> {
        const NAME: f32 = 150.0;
        const COMPRESSION: f32 = 150.0;
        const ENCODING: f32 = 200.0;
        const STATISTICS: f32 = 100.0;
        const FLAG: f32 = 90.0;

        let header = row![
            text("Column").width(NAME),
            text("Compression").width(COMPRESSION),
            text("Encoding").width(ENCODING),
            text("Dictionary").width(FLAG),
            text("Statistics").width(STATISTICS),
            text("Bloom filter").width(FLAG),
        ]
        .spacing(8);

        let settings_row = |i: Option<usize>, label: String, encodings: Option<Vec<Encoding>>| {
            let s = self.column_settings(i);
            let encoding: Element<'_, WriterMessage> = match (encodings, i) {
                (Some(choices), Some(i)) => pick_list(choices, Some(s.encoding), move |e| {
                    WriterMessage::Encoding(i, e)
                })
                .width(ENCODING)
                .into(),
                _ => text("plain")
                    .width(ENCODING)
                    .color(tokens.muted_text)
                    .into(),
            };
            row![
                text(label).width(NAME),
                pick_list(
                    Compression::CHOICES,
                    Some(s.compression.without_level()),
                    move |c| { WriterMessage::Compression(i, c) }
                )
                .width(COMPRESSION),
                encoding,
                checkbox(s.dictionary)
                    .on_toggle(move |on| WriterMessage::Dictionary(i, on))
                    .width(FLAG),
                pick_list(StatisticsLevel::ALL, Some(s.statistics), move |l| {
                    WriterMessage::Statistics(i, l)
                })
                .width(STATISTICS),
                checkbox(s.bloom_filter)
                    .on_toggle(move |on| WriterMessage::Bloom(i, on))
                    .width(FLAG),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
        };

        let mut columns = column![header].spacing(6);
        for (i, (name, data_type)) in self.columns.iter().enumerate() {
            columns = columns.push(settings_row(
                Some(i),
                name.clone(),
                Some(Encoding::choices_for(data_type)),
            ));
        }

        let mut body = column![
            text("Writer settings").size(20),
            text("Used when saving. Opened files keep the settings they were written with.")
                .color(tokens.muted_text),
            row![
                text("Format version").width(NAME),
                pick_list(
                    FormatVersion::ALL,
                    Some(self.settings.format_version),
                    WriterMessage::Version
                )
                .width(COMPRESSION),
                Space::new().width(24),
                text("Rows per row group"),
                text_input("1048576", &self.row_group)
                    .on_input(WriterMessage::RowGroup)
                    .width(140),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text("Columns").size(16),
            scrollable(columns).height(Length::Fixed(260.0)),
            settings_row(None, "New columns".into(), None),
        ]
        .spacing(14);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons(
            "Apply",
            WriterMessage::Cancel,
            WriterMessage::Apply,
        ));
        card(body.into(), tokens, 860.0)
    }
}

/// Edit the file's key/value metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataDialog {
    entries: Vec<(String, String)>,
    /// The Arrow schema entry, kept as is: the writer regenerates it.
    arrow_schema: Option<KeyValue>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MetadataMessage {
    Key(usize, String),
    Value(usize, String),
    Remove(usize),
    Add,
    Apply,
    Cancel,
}

impl MetadataDialog {
    pub fn new(entries: &[KeyValue]) -> Self {
        Self {
            entries: entries
                .iter()
                .filter(|kv| kv.key != ARROW_SCHEMA_KEY)
                .map(|kv| (kv.key.clone(), kv.value.clone().unwrap_or_default()))
                .collect(),
            arrow_schema: entries
                .iter()
                .find(|kv| kv.key == ARROW_SCHEMA_KEY)
                .cloned(),
            error: None,
        }
    }

    /// Handles a message. Returns the command to run on apply.
    pub fn update(&mut self, message: MetadataMessage) -> Option<Command> {
        match message {
            MetadataMessage::Key(i, key) => {
                if let Some(entry) = self.entries.get_mut(i) {
                    entry.0 = key;
                }
            }
            MetadataMessage::Value(i, value) => {
                if let Some(entry) = self.entries.get_mut(i) {
                    entry.1 = value;
                }
            }
            MetadataMessage::Remove(i) => {
                if i < self.entries.len() {
                    self.entries.remove(i);
                }
            }
            MetadataMessage::Add => self.entries.push((String::new(), String::new())),
            MetadataMessage::Cancel => {}
            MetadataMessage::Apply => {
                let mut entries: Vec<KeyValue> = self
                    .entries
                    .iter()
                    // Rows left completely empty are ignored.
                    .filter(|(k, v)| !(k.trim().is_empty() && v.is_empty()))
                    .map(|(k, v)| KeyValue {
                        key: k.trim().to_owned(),
                        value: (!v.is_empty()).then(|| v.clone()),
                    })
                    .collect();
                if entries.iter().any(|kv| kv.key == ARROW_SCHEMA_KEY) {
                    self.error = Some(format!("{ARROW_SCHEMA_KEY} is managed automatically."));
                    return None;
                }
                entries.extend(self.arrow_schema.clone());
                return Some(Command::ReplaceMetadata(entries));
            }
        }
        None
    }

    /// Shows an error from applying the command.
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, MetadataMessage> {
        let mut rows =
            column![row![text("Key").width(220), text("Value").width(Length::Fill)].spacing(8)]
                .spacing(6);
        for (i, (key, value)) in self.entries.iter().enumerate() {
            rows = rows.push(
                row![
                    text_input("key", key)
                        .on_input(move |k| MetadataMessage::Key(i, k))
                        .width(220),
                    text_input("value", value)
                        .on_input(move |v| MetadataMessage::Value(i, v))
                        .width(Length::Fill),
                    button(text("Remove"))
                        .style(button::text)
                        .on_press(MetadataMessage::Remove(i)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if self.entries.is_empty() {
            rows = rows.push(text("No metadata.").color(tokens.muted_text));
        }
        let mut body = column![
            text("File metadata").size(20),
            text("Key/value pairs stored in the file's footer.").color(tokens.muted_text),
            scrollable(rows).height(Length::Fixed(280.0)),
            button(text("Add entry"))
                .style(button::secondary)
                .on_press(MetadataMessage::Add),
        ]
        .spacing(14);
        if self.arrow_schema.is_some() {
            body = body.push(
                text(format!(
                    "{ARROW_SCHEMA_KEY} (the Arrow schema) is not shown: it is rewritten on save."
                ))
                .color(tokens.muted_text),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons(
            "Apply",
            MetadataMessage::Cancel,
            MetadataMessage::Apply,
        ));
        card(body.into(), tokens, 640.0)
    }
}

fn buttons<'a, M: Clone + 'a>(action: &'a str, cancel: M, submit: M) -> Element<'a, M> {
    row![
        Space::new().width(Length::Fill),
        button(text("Cancel"))
            .style(button::secondary)
            .on_press(cancel),
        button(text(action)).on_press(submit),
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_rename_build_commands() {
        let mut add = ColumnDialog::add(2);
        add.update(ColumnMessage::Name(" total ".into()));
        add.update(ColumnMessage::Type(TypeChoice::Decimal));
        assert_eq!(
            add.update(ColumnMessage::Submit),
            Some(Command::AddColumn {
                name: "total".into(),
                data_type: DataType::Float64,
                at: 2
            })
        );
        let mut rename = ColumnDialog::rename("a".into());
        rename.update(ColumnMessage::Name("b".into()));
        assert_eq!(
            rename.update(ColumnMessage::Submit),
            Some(Command::RenameColumn {
                from: "a".into(),
                to: "b".into()
            })
        );
    }

    #[test]
    fn writer_dialog_edits_columns_and_defaults() {
        let mut settings = WriterSettings::default();
        settings
            .columns
            .push(("a".into(), ColumnSettings::default()));
        let mut dialog = WriterDialog::new(
            settings,
            vec![("a".into(), DataType::Int64), ("b".into(), DataType::Utf8)],
        );
        dialog.update(WriterMessage::Compression(Some(0), Compression::Zstd(None)));
        dialog.update(WriterMessage::Encoding(1, Encoding::DeltaByteArray));
        dialog.update(WriterMessage::Dictionary(None, false));
        dialog.update(WriterMessage::Version(FormatVersion::V1));
        dialog.update(WriterMessage::RowGroup("x".into()));
        assert_eq!(dialog.update(WriterMessage::Apply), None);
        assert!(dialog.error.is_some());
        dialog.update(WriterMessage::RowGroup("5000".into()));
        let Some(Command::SetWriterSettings(s)) = dialog.update(WriterMessage::Apply) else {
            panic!("expected settings");
        };
        assert_eq!(s.max_row_group_rows, 5000);
        assert_eq!(s.format_version, FormatVersion::V1);
        assert_eq!(s.column("a").compression, Compression::Zstd(None));
        assert_eq!(s.column("b").encoding, Encoding::DeltaByteArray);
        assert!(!s.default_column.dictionary);
        assert_eq!(s.columns.len(), 2);
    }

    #[test]
    fn metadata_dialog_keeps_arrow_schema_hidden() {
        let kv = |k: &str, v: &str| KeyValue {
            key: k.into(),
            value: Some(v.into()),
        };
        let mut dialog = MetadataDialog::new(&[kv("owner", "me"), kv(ARROW_SCHEMA_KEY, "xyz")]);
        assert_eq!(dialog.entries.len(), 1);
        dialog.update(MetadataMessage::Add);
        dialog.update(MetadataMessage::Key(1, " team ".into()));
        dialog.update(MetadataMessage::Add); // left empty: ignored
        dialog.update(MetadataMessage::Value(0, String::new()));
        let Some(Command::ReplaceMetadata(entries)) = dialog.update(MetadataMessage::Apply) else {
            panic!("expected metadata");
        };
        assert_eq!(
            entries,
            vec![
                KeyValue {
                    key: "owner".into(),
                    value: None
                },
                KeyValue {
                    key: "team".into(),
                    value: None
                },
                kv(ARROW_SCHEMA_KEY, "xyz"),
            ]
        );

        dialog.update(MetadataMessage::Key(0, ARROW_SCHEMA_KEY.into()));
        assert_eq!(dialog.update(MetadataMessage::Apply), None);
        assert!(dialog.error.is_some());
    }

    #[test]
    fn choose_columns_removes_unchecked() {
        let mut choose = ChooseColumns::new(["a", "b", "c"].map(String::from));
        assert_eq!(choose.update(ChooseMessage::Apply), Some(None));
        choose.update(ChooseMessage::Toggle(1, false));
        choose.update(ChooseMessage::All(false));
        choose.update(ChooseMessage::Toggle(0, true));
        assert_eq!(
            choose.update(ChooseMessage::Apply),
            Some(Some(Command::RemoveColumns {
                names: vec!["b".into(), "c".into()]
            }))
        );
    }
}
