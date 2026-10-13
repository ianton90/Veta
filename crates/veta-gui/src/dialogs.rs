//! Dialogs for column commands (add, rename, choose columns, change type,
//! replace values, format text), filters, sorts, writer settings, and CSV
//! import / export.

use iced::widget::{Space, button, checkbox, column, pick_list, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use veta_core::arrow::array::ArrayRef;
use veta_core::arrow::datatypes::SchemaRef;
use veta_core::arrow::datatypes::{DataType, TimeUnit};
use veta_core::convert::{
    CsvExportOptions, CsvOptions, CsvPreview, ExportFormat, TextEncoding, preview_csv,
};
use veta_core::display::type_name;
use veta_core::io::ARROW_SCHEMA_KEY;
use veta_core::model::KeyValue;
use veta_core::steps::{
    Cast, CastOptions, CastPreview, Condition, FORMAT_CODES, Filter, FilterOp, OnError, Replace,
    Sort, SortKey, TextOp,
};
use veta_core::steps::{CustomColumn, Merge, Split, SplitBy, count_parts, expression_help};
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
    pub const ALL: [TypeChoice; 6] = [
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

/// A target type offered when changing a column's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastTarget {
    Text,
    Int64,
    Int32,
    Float64,
    Float32,
    Boolean,
    Date,
    Timestamp,
    Time,
    /// Any type, typed by name.
    Other,
}

impl CastTarget {
    pub const ALL: [CastTarget; 10] = [
        Self::Text,
        Self::Int64,
        Self::Int32,
        Self::Float64,
        Self::Float32,
        Self::Boolean,
        Self::Date,
        Self::Timestamp,
        Self::Time,
        Self::Other,
    ];

    fn data_type(self) -> Option<DataType> {
        Some(match self {
            Self::Text => DataType::Utf8,
            Self::Int64 => DataType::Int64,
            Self::Int32 => DataType::Int32,
            Self::Float64 => DataType::Float64,
            Self::Float32 => DataType::Float32,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
            Self::Time => DataType::Time64(TimeUnit::Microsecond),
            Self::Other => return None,
        })
    }
}

impl std::fmt::Display for CastTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text => "Text (string)",
            Self::Int64 => "Whole number (int64)",
            Self::Int32 => "Whole number (int32)",
            Self::Float64 => "Decimal number (float64)",
            Self::Float32 => "Decimal number (float32)",
            Self::Boolean => "True/false (bool)",
            Self::Date => "Date",
            Self::Timestamp => "Date and time (timestamp)",
            Self::Time => "Time of day (time)",
            Self::Other => "Other type…",
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

/// Pick columns to keep or to remove.
#[derive(Debug, Clone, PartialEq)]
pub struct ChooseColumns {
    /// Keep the checked columns (otherwise remove them).
    keep: bool,
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
    /// `checked` columns start checked; with none, all start checked when
    /// keeping and unchecked when removing.
    pub fn new(keep: bool, names: impl IntoIterator<Item = String>, checked: &[String]) -> Self {
        let columns = names
            .into_iter()
            .map(|n| {
                let on = if checked.is_empty() {
                    keep
                } else {
                    checked.contains(&n)
                };
                (n, on)
            })
            .collect();
        Self { keep, columns }
    }

    /// Handles a message. Returns the command to run on apply, if it changes
    /// anything.
    pub fn update(&mut self, message: ChooseMessage) -> Option<Option<Command>> {
        match message {
            ChooseMessage::Toggle(i, on) => {
                if let Some(column) = self.columns.get_mut(i) {
                    column.1 = on;
                }
            }
            ChooseMessage::All(on) => {
                for column in &mut self.columns {
                    column.1 = on;
                }
            }
            ChooseMessage::Cancel => {}
            ChooseMessage::Apply => {
                let names: Vec<String> = self
                    .columns
                    .iter()
                    .filter(|(_, on)| *on)
                    .map(|(name, _)| name.clone())
                    .collect();
                let command = if self.keep {
                    (names.len() < self.columns.len()).then_some(Command::KeepColumns { names })
                } else {
                    (!names.is_empty()).then_some(Command::RemoveColumns { names })
                };
                return Some(command);
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ChooseMessage> {
        let all = self.columns.iter().all(|(_, on)| *on);
        let list = column(self.columns.iter().enumerate().map(|(i, (name, on))| {
            checkbox(*on)
                .label(name.as_str())
                .on_toggle(move |on| ChooseMessage::Toggle(i, on))
                .into()
        }))
        .spacing(6);
        let checked = self.columns.iter().filter(|(_, on)| *on).count();
        let (title, summary) = if self.keep {
            (
                "Keep columns",
                format!("{checked} of {} columns kept", self.columns.len()),
            )
        } else {
            (
                "Remove columns",
                format!("{checked} of {} columns removed", self.columns.len()),
            )
        };
        let body = column![
            text(title).size(20),
            checkbox(all)
                .label("Select all")
                .on_toggle(ChooseMessage::All),
            scrollable(list).height(Length::Fixed(320.0)),
            text(summary).color(tokens.muted_text),
            buttons("OK", ChooseMessage::Cancel, ChooseMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 420.0)
    }
}

/// Id of the first condition's value field, focused when the dialog opens.
pub const FILTER_VALUE_ID: iced::widget::Id = iced::widget::Id::new("filter-value");

/// Operators that apply to nested (struct, list, map) columns: equality on
/// the JSON value, text tests on its JSON text, and null tests.
const NESTED_OPS: [FilterOp; 8] = [
    FilterOp::Equals,
    FilterOp::NotEquals,
    FilterOp::Contains,
    FilterOp::NotContains,
    FilterOp::StartsWith,
    FilterOp::EndsWith,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
];

/// Keep rows that match conditions (Power Query's "Filter Rows").
#[derive(Debug, Clone, PartialEq)]
pub struct FilterDialog {
    columns: Vec<String>,
    /// Columns with nested types.
    nested: Vec<String>,
    conditions: Vec<Condition>,
    matching: Matching,
    /// Kept from an edited step: remove the matching rows.
    exclude: bool,
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
            nested: Vec::new(),
            conditions: vec![Condition {
                column,
                op: FilterOp::Equals,
                value: String::new(),
                case_sensitive: false,
            }],
            matching: Matching::All,
            exclude: false,
            error: None,
        }
    }

    /// A dialog editing an existing filter.
    pub fn edit(columns: Vec<String>, filter: &Filter) -> Self {
        Self {
            columns,
            nested: Vec::new(),
            conditions: filter.conditions.clone(),
            matching: if filter.match_all {
                Matching::All
            } else {
                Matching::Any
            },
            exclude: filter.exclude,
            error: None,
        }
    }

    /// Marks the columns with nested types (they offer fewer operators).
    pub fn with_nested(mut self, nested: Vec<String>) -> Self {
        self.nested = nested;
        self
    }

    fn is_nested(&self, column: &str) -> bool {
        self.nested.iter().any(|n| n == column)
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Handles a message. Returns the filter to apply on submit.
    pub fn update(&mut self, message: FilterMessage) -> Option<Filter> {
        self.error = None;
        match message {
            FilterMessage::Column(i, column) => {
                let nested = self.is_nested(&column);
                self.edit_condition(i, |c| {
                    c.column = column;
                    if nested && !NESTED_OPS.contains(&c.op) {
                        c.op = FilterOp::Equals;
                    }
                });
            }
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
                    exclude: self.exclude,
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
            let nested = self.is_nested(&c.column);
            let placeholder = if nested { "JSON, e.g. [1, 2]" } else { "Value" };
            let mut value = text_input(placeholder, &c.value).width(Length::Fill);
            if i == 0 {
                value = value.id(FILTER_VALUE_ID);
            }
            let ops: Vec<FilterOp> = if nested {
                NESTED_OPS.to_vec()
            } else {
                FilterOp::ALL.to_vec()
            };
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
                    pick_list(ops, Some(c.op), move |op| FilterMessage::Op(i, op)).width(190),
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
            text(
                "Values are read as the column's type; nested columns compare as JSON. \
                 \"Aa\" matches text case-sensitively."
            )
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
    /// The column's values are numbers (text output can use separators).
    from_number: bool,
    target: CastTarget,
    /// Type name typed for "Other type…", e.g. `decimal(10, 2)`.
    other: String,
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
    Type(CastTarget),
    Other(String),
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
        let from_number = sample.data_type().is_numeric();
        let options = cast.map(|c| c.options.clone()).unwrap_or_default();
        let (target, other) = match cast {
            Some(c) => match CastTarget::ALL
                .iter()
                .find(|t| t.data_type().as_ref() == Some(&c.to))
            {
                Some(t) => (*t, String::new()),
                None => (CastTarget::Other, veta_core::display::type_name(&c.to)),
            },
            None if from_text => (CastTarget::Float64, String::new()),
            None => (CastTarget::Text, String::new()),
        };
        let mut dialog = Self {
            column,
            from_text,
            from_number,
            target,
            other,
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

    /// The chosen type; `None` while "Other type…" holds no valid name.
    fn data_type(&self) -> Option<DataType> {
        match self.target {
            CastTarget::Other => veta_core::display::parse_type_name(&self.other),
            target => target.data_type(),
        }
    }

    fn uses_format(&self) -> bool {
        self.from_text
            && matches!(
                self.data_type(),
                Some(DataType::Date32 | DataType::Timestamp(..))
            )
    }

    fn uses_separators(&self) -> bool {
        let to = self.data_type();
        let to_number = to.as_ref().is_some_and(DataType::is_numeric);
        let to_text = to.as_ref().is_some_and(is_text);
        (self.from_text && to_number) || (self.from_number && to_text)
    }

    fn cast(&self) -> Option<Cast> {
        let format = self.format.trim();
        Some(Cast {
            column: self.column.clone(),
            to: self.data_type()?,
            options: CastOptions {
                decimal: self.decimal.char().unwrap_or('.'),
                thousands: self.thousands.char(),
                format: (self.uses_format() && !format.is_empty()).then(|| format.to_owned()),
                on_error: self.on_error,
            },
        })
    }

    fn refresh(&mut self) {
        self.preview = match self.cast() {
            Some(cast) => cast.preview(&self.sample),
            None => CastPreview::default(),
        };
    }

    /// Handles a message. Returns the cast to apply on submit.
    pub fn update(&mut self, message: CastMessage) -> Option<Cast> {
        match message {
            CastMessage::Type(target) => {
                if target == CastTarget::Other && self.other.is_empty() {
                    self.other = self
                        .data_type()
                        .map(|t| veta_core::display::type_name(&t))
                        .unwrap_or_default();
                }
                self.target = target;
            }
            CastMessage::Other(name) => self.other = name,
            CastMessage::Decimal(s) => self.decimal = s,
            CastMessage::Thousands(s) => self.thousands = s,
            CastMessage::Format(f) => self.format = f,
            CastMessage::OnError(e) => self.on_error = e,
            CastMessage::Cancel => return None,
            CastMessage::Apply => return self.cast(),
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
                pick_list(CastTarget::ALL, Some(self.target), CastMessage::Type)
                    .width(Length::Fill)
                    .into()
            ),
        ]
        .spacing(14);
        if self.target == CastTarget::Other {
            body = body.push(labelled(
                "Type name",
                text_input(
                    "e.g. decimal(10, 2), timestamp[ms, UTC], list<int64>",
                    &self.other,
                )
                .on_input(CastMessage::Other)
                .on_submit(CastMessage::Apply)
                .into(),
            ));
            if self.data_type().is_none() {
                body = body.push(
                    text(
                        "Types: int8-64, uint8-64, float32/64, decimal(p, s), bool, string, \
                         binary, date, time[s|ms|us|ns], timestamp[unit] or \
                         timestamp[unit, zone], duration[unit], list<type>.",
                    )
                    .size(12)
                    .color(tokens.muted_text),
                );
            }
        }
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
        let summary = if self.data_type().is_none() {
            "Enter a type name.".to_owned()
        } else if p.checked == 0 {
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

/// Add a column computed from an expression.
#[derive(Debug, Clone)]
pub struct CustomDialog {
    /// The input of the new column's step.
    schema: SchemaRef,
    name: String,
    expression: String,
    at: usize,
    /// Type of the expression, or why it is invalid.
    check: Result<String, String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CustomMessage {
    Name(String),
    Expression(String),
    Apply,
    Cancel,
}

/// Id of the expression input.
pub const EXPRESSION_ID: iced::widget::Id = iced::widget::Id::new("expression");

impl CustomDialog {
    pub fn new(schema: SchemaRef, custom: CustomColumn) -> Self {
        let mut dialog = Self {
            schema,
            name: custom.name,
            expression: custom.expression,
            at: custom.at,
            check: Ok(String::new()),
            error: None,
        };
        dialog.recheck();
        dialog
    }

    fn custom(&self) -> CustomColumn {
        CustomColumn {
            name: self.name.trim().to_owned(),
            expression: self.expression.clone(),
            at: self.at,
        }
    }

    fn recheck(&mut self) {
        self.check = if self.expression.trim().is_empty() {
            Err(String::new())
        } else {
            self.custom()
                .data_type(&self.schema)
                .map(|t| type_name(&t))
                .map_err(|e| e.to_string())
        };
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: CustomMessage) -> Option<CustomColumn> {
        self.error = None;
        match message {
            CustomMessage::Name(n) => self.name = n,
            CustomMessage::Expression(e) => {
                self.expression = e;
                self.recheck();
            }
            CustomMessage::Cancel => {}
            CustomMessage::Apply => return Some(self.custom()),
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, CustomMessage> {
        let danger = iced::Color::from_rgb8(0xd1, 0x43, 0x43);
        let status: Element<'_, CustomMessage> = match &self.check {
            Ok(t) => text(format!("Result: {t}")).color(tokens.muted_text).into(),
            Err(e) if e.is_empty() => text("").into(),
            Err(e) => text(e).color(danger).into(),
        };
        let columns: Vec<String> = self
            .schema
            .fields()
            .iter()
            .map(|f| {
                if f.name().chars().all(|c| c.is_alphanumeric() || c == '_') {
                    f.name().clone()
                } else {
                    format!("[{}]", f.name())
                }
            })
            .collect();
        let mut body = column![
            text("Custom column").size(20),
            row![
                text("Name").width(90),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(CustomMessage::Name)
                    .on_submit(CustomMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text("Formula").width(90),
                text_input("e.g. [Unit price] * qty", &self.expression)
                    .id(EXPRESSION_ID)
                    .on_input(CustomMessage::Expression)
                    .on_submit(CustomMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            status,
            text(format!("Columns: {}", columns.join(", ")))
                .size(12)
                .color(tokens.muted_text),
            text(expression_help()).size(12).color(tokens.muted_text),
        ]
        .spacing(12);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(danger));
        }
        body = body.push(buttons("OK", CustomMessage::Cancel, CustomMessage::Apply));
        card(body.into(), tokens, 620.0)
    }
}

/// Field separators offered for CSV files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
    Pipe,
    /// Typed by the user.
    Other,
}

impl Delimiter {
    pub const ALL: [Delimiter; 5] = [
        Self::Comma,
        Self::Semicolon,
        Self::Tab,
        Self::Pipe,
        Self::Other,
    ];

    fn byte(self) -> Option<u8> {
        Some(match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
            Self::Pipe => b'|',
            Self::Other => return None,
        })
    }
}

/// The separator byte: a preset, or the single ASCII character typed.
fn separator(delimiter: Delimiter, other: &str) -> Result<u8, String> {
    if let Some(b) = delimiter.byte() {
        return Ok(b);
    }
    match other.as_bytes() {
        [b] if b.is_ascii() && !b.is_ascii_alphanumeric() && *b != b'\n' => Ok(*b),
        _ => Err("Type one punctuation character as the separator.".into()),
    }
}

/// Quote characters (text qualifiers) for CSV files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    Double,
    Single,
    None,
}

impl Quote {
    pub const ALL: [Quote; 3] = [Self::Double, Self::Single, Self::None];

    fn byte(self) -> Option<u8> {
        match self {
            Self::Double => Some(b'"'),
            Self::Single => Some(b'\''),
            Self::None => None,
        }
    }
}

impl std::fmt::Display for Quote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Double => "double ( \" )",
            Self::Single => "single ( ' )",
            Self::None => "none",
        })
    }
}

impl std::fmt::Display for Delimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Comma => "comma ( , )",
            Self::Semicolon => "semicolon ( ; )",
            Self::Tab => "tab",
            Self::Pipe => "pipe ( | )",
            Self::Other => "other…",
        })
    }
}

/// Decimal separators, for the pick lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalMark {
    Dot,
    Comma,
}

impl DecimalMark {
    fn char(self) -> char {
        match self {
            Self::Dot => '.',
            Self::Comma => ',',
        }
    }
}

impl std::fmt::Display for DecimalMark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dot => "dot (1.5)",
            Self::Comma => "comma (1,5)",
        })
    }
}

/// Column types offered when importing CSV.
const IMPORT_TYPES: [CastTarget; 9] = [
    CastTarget::Text,
    CastTarget::Int64,
    CastTarget::Int32,
    CastTarget::Float64,
    CastTarget::Float32,
    CastTarget::Boolean,
    CastTarget::Date,
    CastTarget::Timestamp,
    CastTarget::Time,
];

/// Rows shown in the import preview.
const PREVIEW_ROWS: usize = 12;

/// Import a CSV file: options, a preview and column types.
#[derive(Debug, Clone)]
pub struct ImportDialog {
    pub path: std::path::PathBuf,
    delimiter: Delimiter,
    /// The separator typed when `delimiter` is `Other`.
    other: String,
    quote: Quote,
    header: bool,
    encoding: TextEncoding,
    decimal: DecimalMark,
    /// Column types chosen (initially the guessed ones).
    types: Vec<DataType>,
    /// Format of date and timestamp columns; empty reads ISO 8601.
    date_format: String,
    on_error: OnError,
    preview: Result<CsvPreview, String>,
}

#[derive(Debug, Clone)]
pub enum ImportMessage {
    Delimiter(Delimiter),
    Other(String),
    Quote(Quote),
    Header(bool),
    Encoding(TextEncoding),
    Decimal(DecimalMark),
    Type(usize, CastTarget),
    DateFormat(String),
    OnError(OnError),
    Import,
    Cancel,
}

impl ImportDialog {
    pub fn new(path: std::path::PathBuf) -> Self {
        let tab = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
        let mut dialog = Self {
            path,
            delimiter: if tab {
                Delimiter::Tab
            } else {
                Delimiter::Comma
            },
            other: String::new(),
            quote: Quote::Double,
            header: true,
            encoding: TextEncoding::Utf8,
            decimal: DecimalMark::Dot,
            types: Vec::new(),
            date_format: String::new(),
            on_error: OnError::ErrorColumn,
            preview: Err(String::new()),
        };
        if !tab {
            dialog.guess_delimiter();
        }
        dialog.refresh(true);
        dialog
    }

    /// Picks the separator that splits the first line into most fields.
    fn guess_delimiter(&mut self) {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return;
        };
        let mut head = vec![0u8; 4096];
        let n = std::io::Read::read(&mut file, &mut head).unwrap_or(0);
        let first = head[..n].split(|&b| b == b'\n').next().unwrap_or_default();
        if let Some(best) = Delimiter::ALL
            .into_iter()
            .filter_map(|d| Some((d, d.byte()?)))
            .max_by_key(|(_, b)| first.iter().filter(|&&c| c == *b).count())
            .filter(|(_, b)| first.contains(b))
            .map(|(d, _)| d)
        {
            self.delimiter = best;
            if best == Delimiter::Semicolon {
                self.decimal = DecimalMark::Comma;
            }
        }
    }

    pub fn options(&self) -> CsvOptions {
        CsvOptions {
            delimiter: separator(self.delimiter, &self.other).unwrap_or(b','),
            quote: self.quote.byte(),
            header: self.header,
            encoding: self.encoding,
            decimal: self.decimal.char(),
            types: (!self.types.is_empty()).then(|| self.types.clone()),
            date_format: Some(self.date_format.trim().to_owned()).filter(|f| !f.is_empty()),
            on_error: self.on_error,
        }
    }

    fn has_dates(&self) -> bool {
        self.types
            .iter()
            .any(|t| matches!(t, DataType::Date32 | DataType::Timestamp(..)))
    }

    /// Re-reads the preview; `guess` drops chosen types and guesses again.
    fn refresh(&mut self, guess: bool) {
        if guess {
            self.types.clear();
        }
        if let Err(e) = separator(self.delimiter, &self.other) {
            self.preview = Err(e);
            return;
        }
        self.preview =
            preview_csv(&self.path, &self.options(), PREVIEW_ROWS).map_err(|e| e.to_string());
        if let Ok(preview) = &self.preview {
            self.types = preview
                .schema
                .fields()
                .iter()
                .map(|f| f.data_type().clone())
                .collect();
        }
    }

    /// Handles a message. Returns the options to import with on submit.
    pub fn update(&mut self, message: ImportMessage) -> Option<CsvOptions> {
        match message {
            ImportMessage::Delimiter(d) => {
                self.delimiter = d;
                self.refresh(true);
            }
            ImportMessage::Other(text) => {
                self.other = text;
                self.refresh(true);
            }
            ImportMessage::Quote(q) => {
                self.quote = q;
                self.refresh(true);
            }
            ImportMessage::Header(on) => {
                self.header = on;
                self.refresh(true);
            }
            ImportMessage::Encoding(e) => {
                self.encoding = e;
                self.refresh(false);
            }
            ImportMessage::Decimal(d) => {
                self.decimal = d;
                self.refresh(true);
            }
            ImportMessage::Type(i, target) => {
                if let (Some(t), Some(to)) = (self.types.get_mut(i), target.data_type()) {
                    *t = to;
                }
                self.refresh(false);
            }
            ImportMessage::DateFormat(format) => {
                self.date_format = format;
                self.refresh(false);
            }
            ImportMessage::OnError(e) => self.on_error = e,
            ImportMessage::Cancel => {}
            ImportMessage::Import => {
                return self.preview.is_ok().then(|| self.options());
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ImportMessage> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let options = row![
            column![
                text("Separator").size(13),
                row![
                    pick_list(
                        Delimiter::ALL,
                        Some(self.delimiter),
                        ImportMessage::Delimiter
                    )
                    .width(160),
                ]
                .extend((self.delimiter == Delimiter::Other).then(|| {
                    text_input("", &self.other)
                        .on_input(ImportMessage::Other)
                        .width(36)
                        .into()
                }))
                .spacing(4),
            ]
            .spacing(4),
            column![
                text("Quote").size(13),
                pick_list(Quote::ALL, Some(self.quote), ImportMessage::Quote).width(130),
            ]
            .spacing(4),
            column![
                text("Decimal").size(13),
                pick_list(
                    [DecimalMark::Dot, DecimalMark::Comma],
                    Some(self.decimal),
                    ImportMessage::Decimal
                )
                .width(140),
            ]
            .spacing(4),
            column![
                text("Encoding").size(13),
                pick_list(
                    TextEncoding::ALL,
                    Some(self.encoding),
                    ImportMessage::Encoding
                )
                .width(150),
            ]
            .spacing(4),
            column![
                text(" ").size(13),
                checkbox(self.header)
                    .label("Header row")
                    .on_toggle(ImportMessage::Header),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .align_y(Alignment::End);
        let preview: Element<'_, ImportMessage> = match &self.preview {
            Err(e) => text(if e.is_empty() {
                "Reading…"
            } else {
                e.as_str()
            })
            .color(iced::Color::from_rgb8(0xd1, 0x43, 0x43))
            .into(),
            Ok(preview) => {
                const WIDTH: f32 = 150.0;
                let cells = veta_core::display::format_batch(&preview.rows).unwrap_or_default();
                let mut header = row![].spacing(6);
                for (i, field) in preview.schema.fields().iter().enumerate() {
                    let choice = IMPORT_TYPES
                        .into_iter()
                        .find(|t| t.data_type().as_ref() == Some(field.data_type()));
                    let picker: Element<'_, ImportMessage> = match choice {
                        Some(c) => {
                            pick_list(IMPORT_TYPES, Some(c), move |t| ImportMessage::Type(i, t))
                                .text_size(12)
                                .width(WIDTH)
                                .into()
                        }
                        None => pick_list(IMPORT_TYPES, None::<CastTarget>, move |t| {
                            ImportMessage::Type(i, t)
                        })
                        .placeholder(veta_core::display::type_name(field.data_type()))
                        .text_size(12)
                        .width(WIDTH)
                        .into(),
                    };
                    let failed = preview.failures.get(i).copied().unwrap_or(0);
                    let note = if failed > 0 {
                        text(format!("{failed} don't fit"))
                            .size(11)
                            .color(iced::Color::from_rgb8(0xd1, 0x43, 0x43))
                    } else {
                        text(" ").size(11)
                    };
                    header = header.push(
                        column![
                            text(field.name().clone()).size(13).width(WIDTH),
                            picker,
                            note
                        ]
                        .spacing(2),
                    );
                }
                let mut grid = column![header].spacing(3);
                for line in cells {
                    let mut r = row![].spacing(6);
                    for value in line {
                        let (value, color) = match value {
                            Some(v) => (v, tokens.text),
                            None => ("null".into(), tokens.muted_text),
                        };
                        r = r.push(text(value).size(12).color(color).width(WIDTH));
                    }
                    grid = grid.push(r);
                }
                scrollable(grid)
                    .direction(scrollable::Direction::Both {
                        vertical: scrollable::Scrollbar::default(),
                        horizontal: scrollable::Scrollbar::default(),
                    })
                    .height(Length::Fixed(300.0))
                    .into()
            }
        };
        let date_format: Option<Element<'_, ImportMessage>> = self.has_dates().then(|| {
            column![
                row![
                    text("Date format"),
                    text_input("Automatic (ISO 8601: 2024-12-31)", &self.date_format)
                        .on_input(ImportMessage::DateFormat)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                text(FORMAT_CODES).size(12).color(tokens.muted_text),
            ]
            .spacing(4)
            .into()
        });
        let body = column![
            text(format!("Import {name}")).size(20),
            options,
            text("Preview").size(15),
            preview,
        ]
        .extend(date_format)
        .push(
            row![
                text("Values that don't fit their column's type:"),
                pick_list(OnError::ALL, Some(self.on_error), ImportMessage::OnError)
                    .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .push(
            text("You'll be told how many values didn't fit after importing.")
                .size(12)
                .color(tokens.muted_text),
        )
        .push(buttons(
            "Import",
            ImportMessage::Cancel,
            ImportMessage::Import,
        ))
        .spacing(14);
        card(body.into(), tokens, 820.0)
    }
}

/// Formats offered for exporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportChoice {
    Csv,
    JsonLines,
    JsonArray,
}

impl std::fmt::Display for ExportChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Csv => "CSV",
            Self::JsonLines => "JSON, one object per line",
            Self::JsonArray => "JSON array",
        })
    }
}

/// Export the document (after all steps) to CSV or JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDialog {
    pub choice: ExportChoice,
    delimiter: Delimiter,
    other: String,
    quote: Quote,
    error: Option<String>,
    decimal: DecimalMark,
    header: bool,
}

#[derive(Debug, Clone)]
pub enum ExportMessage {
    Format(ExportChoice),
    Delimiter(Delimiter),
    Other(String),
    Quote(Quote),
    Decimal(DecimalMark),
    Header(bool),
    Export,
    Cancel,
}

impl Default for ExportDialog {
    fn default() -> Self {
        Self {
            choice: ExportChoice::Csv,
            delimiter: Delimiter::Comma,
            other: String::new(),
            quote: Quote::Double,
            error: None,
            decimal: DecimalMark::Dot,
            header: true,
        }
    }
}

impl ExportDialog {
    pub fn format(&self) -> ExportFormat {
        match self.choice {
            ExportChoice::Csv => ExportFormat::Csv(CsvExportOptions {
                delimiter: separator(self.delimiter, &self.other).unwrap_or(b','),
                quote: self.quote.byte(),
                header: self.header,
                decimal: self.decimal.char(),
            }),
            ExportChoice::JsonLines => ExportFormat::JsonLines,
            ExportChoice::JsonArray => ExportFormat::JsonArray,
        }
    }

    /// Handles a message. Returns the format on submit.
    pub fn update(&mut self, message: ExportMessage) -> Option<ExportFormat> {
        self.error = None;
        match message {
            ExportMessage::Format(c) => self.choice = c,
            ExportMessage::Other(text) => self.other = text,
            ExportMessage::Quote(q) => self.quote = q,
            ExportMessage::Delimiter(d) => {
                self.delimiter = d;
                if d == Delimiter::Semicolon {
                    self.decimal = DecimalMark::Comma;
                }
            }
            ExportMessage::Decimal(d) => self.decimal = d,
            ExportMessage::Header(on) => self.header = on,
            ExportMessage::Cancel => {}
            ExportMessage::Export => {
                if self.choice == ExportChoice::Csv
                    && let Err(e) = separator(self.delimiter, &self.other)
                {
                    self.error = Some(e);
                    return None;
                }
                return Some(self.format());
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ExportMessage> {
        let labelled = |label: &'static str, control: Element<'static, ExportMessage>| {
            row![text(label).width(110), control]
                .spacing(12)
                .align_y(Alignment::Center)
        };
        let mut body = column![
            text("Export").size(20),
            labelled(
                "Format",
                pick_list(
                    [
                        ExportChoice::Csv,
                        ExportChoice::JsonLines,
                        ExportChoice::JsonArray
                    ],
                    Some(self.choice),
                    ExportMessage::Format
                )
                .width(Length::Fill)
                .into()
            ),
        ]
        .spacing(14);
        if self.choice == ExportChoice::Csv {
            body = body
                .push(labelled(
                    "Separator",
                    row![
                        pick_list(
                            Delimiter::ALL,
                            Some(self.delimiter),
                            ExportMessage::Delimiter,
                        )
                        .width(Length::Fill)
                    ]
                    .extend((self.delimiter == Delimiter::Other).then(|| {
                        text_input("", &self.other)
                            .on_input(ExportMessage::Other)
                            .width(36)
                            .into()
                    }))
                    .spacing(4)
                    .into(),
                ))
                .push(labelled(
                    "Quote",
                    pick_list(Quote::ALL, Some(self.quote), ExportMessage::Quote)
                        .width(Length::Fill)
                        .into(),
                ))
                .push(labelled(
                    "Decimal",
                    pick_list(
                        [DecimalMark::Dot, DecimalMark::Comma],
                        Some(self.decimal),
                        ExportMessage::Decimal,
                    )
                    .width(Length::Fill)
                    .into(),
                ))
                .push(
                    checkbox(self.header)
                        .label("Write column names in the first row")
                        .on_toggle(ExportMessage::Header),
                );
        }
        body = body
            .push(text("Exports the data after all applied steps.").color(tokens.muted_text))
            .push(buttons(
                "Export…",
                ExportMessage::Cancel,
                ExportMessage::Export,
            ));
        card(body.into(), tokens, 460.0)
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
    Nulls(usize, Nulls),
    Add,
    Remove(usize),
    Apply,
    Cancel,
}

/// Where nulls go, for the pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nulls {
    Last,
    First,
}

impl std::fmt::Display for Nulls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Last => "nulls last",
            Self::First => "nulls first",
        })
    }
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
            SortMessage::Nulls(i, nulls) => {
                if let Some(key) = self.keys.get_mut(i) {
                    key.nulls_first = nulls == Nulls::First;
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
                    nulls_first: false,
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
                    pick_list(
                        [Nulls::Last, Nulls::First],
                        Some(if key.nulls_first {
                            Nulls::First
                        } else {
                            Nulls::Last
                        }),
                        move |n| SortMessage::Nulls(i, n)
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
            text("Rows that tie keep their order.").color(tokens.muted_text),
            buttons("OK", SortMessage::Cancel, SortMessage::Apply),
        ]
        .spacing(14);
        card(body.into(), tokens, 620.0)
    }
}

/// A compression level choice: `None` is the codec's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level(pub Option<i32>);

impl Level {
    /// Levels offered for `compression` (empty if it takes none).
    pub fn choices(compression: Compression) -> Vec<Level> {
        let mut choices = vec![Level(None)];
        if let Some(range) = compression.level_range() {
            choices.extend(range.map(|l| Level(Some(l))));
        }
        choices
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(level) => write!(f, "{level}"),
            None => f.write_str("default"),
        }
    }
}

/// Edit the settings used when saving: file-wide settings, defaults for new
/// columns, and per-column settings.
#[derive(Debug, Clone, PartialEq)]
pub struct WriterDialog {
    settings: WriterSettings,
    /// Leaf columns (dotted Parquet paths) and their types, in order.
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
    Level(Option<usize>, Level),
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
            WriterMessage::Compression(i, c) => self.edit(i, |s| {
                // Keep the level when switching between codecs that take one.
                let level = s
                    .compression
                    .level()
                    .filter(|l| c.level_range().is_some_and(|r| r.contains(l)));
                s.compression = c.with_level(level);
            }),
            WriterMessage::Level(i, level) => {
                self.edit(i, |s| s.compression = s.compression.with_level(level.0));
            }
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
        const NAME: f32 = 160.0;
        const COMPRESSION: f32 = 145.0;
        const LEVEL: f32 = 90.0;
        const ENCODING: f32 = 185.0;
        const STATISTICS: f32 = 90.0;
        const FLAG: f32 = 85.0;

        let header = row![
            text("Column").width(NAME),
            text("Compression").width(COMPRESSION),
            text("Level").width(LEVEL),
            text("Encoding").width(ENCODING),
            text("Dictionary").width(FLAG),
            text("Statistics").width(STATISTICS),
            text("Bloom").width(FLAG),
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
                level_picker(s.compression, LEVEL, move |l| WriterMessage::Level(i, l)),
                encoding,
                iced::widget::container(
                    checkbox(s.dictionary).on_toggle(move |on| WriterMessage::Dictionary(i, on))
                )
                .width(FLAG),
                pick_list(StatisticsLevel::ALL, Some(s.statistics), move |l| {
                    WriterMessage::Statistics(i, l)
                })
                .width(STATISTICS),
                iced::widget::container(
                    checkbox(s.bloom_filter).on_toggle(move |on| WriterMessage::Bloom(i, on))
                )
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
            scrollable(columns.padding(iced::Padding::ZERO.right(16))).height(Length::Fixed(260.0)),
            settings_row(None, "New columns".into(), None),
            text(
                "Nested columns are listed per leaf. With a dictionary, the encoding is used \
                 only if the dictionary grows too large; files record it only then, so it \
                 shows as plain after reopening."
            )
            .size(13)
            .color(tokens.muted_text),
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
        card(body.into(), tokens, 940.0)
    }
}

/// A level pick list for `compression`, or a dash for codecs without levels.
pub fn level_picker<'a, M: Clone + 'a>(
    compression: Compression,
    width: f32,
    on_pick: impl Fn(Level) -> M + 'a,
) -> Element<'a, M> {
    if compression.level_range().is_none() {
        return iced::widget::container(text("—"))
            .width(width)
            .align_y(Alignment::Center)
            .into();
    }
    pick_list(
        Level::choices(compression),
        Some(Level(compression.level())),
        on_pick,
    )
    .width(width)
    .into()
}

/// Edit key/value metadata: the file's (footer) and each column's (stored
/// in the schema).
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataDialog {
    /// Scope 0 is the file; the others are columns.
    scopes: Vec<MetadataScope>,
    current: usize,
    /// The Arrow schema entry, kept as is: the writer regenerates it.
    arrow_schema: Option<KeyValue>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct MetadataScope {
    /// `None` for the file.
    column: Option<String>,
    original: Vec<(String, String)>,
    entries: Vec<(String, String)>,
}

/// A choice in the scope picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChoice(usize, String);

impl std::fmt::Display for ScopeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

#[derive(Debug, Clone)]
pub enum MetadataMessage {
    Scope(ScopeChoice),
    Key(usize, String),
    Value(usize, String),
    Remove(usize),
    Add,
    Apply,
    Cancel,
}

fn pairs(entries: &[KeyValue]) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|kv| kv.key != ARROW_SCHEMA_KEY)
        .map(|kv| (kv.key.clone(), kv.value.clone().unwrap_or_default()))
        .collect()
}

impl MetadataDialog {
    /// `columns` are the top-level columns with their current metadata.
    pub fn new(file: &[KeyValue], columns: Vec<(String, Vec<KeyValue>)>) -> Self {
        let file_entries = pairs(file);
        let mut scopes = vec![MetadataScope {
            column: None,
            original: file_entries.clone(),
            entries: file_entries,
        }];
        scopes.extend(columns.into_iter().map(|(name, entries)| {
            let entries = pairs(&entries);
            MetadataScope {
                column: Some(name),
                original: entries.clone(),
                entries,
            }
        }));
        Self {
            scopes,
            current: 0,
            arrow_schema: file.iter().find(|kv| kv.key == ARROW_SCHEMA_KEY).cloned(),
            error: None,
        }
    }

    fn choices(&self) -> Vec<ScopeChoice> {
        self.scopes
            .iter()
            .enumerate()
            .map(|(i, scope)| {
                let label = match &scope.column {
                    None => "File".to_owned(),
                    Some(name) => format!("Column: {name}"),
                };
                ScopeChoice(i, label)
            })
            .collect()
    }

    fn entries_mut(&mut self) -> &mut Vec<(String, String)> {
        &mut self.scopes[self.current].entries
    }

    /// Handles a message. Returns the commands to run on apply (one per
    /// changed scope; none if nothing changed).
    pub fn update(&mut self, message: MetadataMessage) -> Option<Vec<Command>> {
        match message {
            MetadataMessage::Scope(choice) => {
                if choice.0 < self.scopes.len() {
                    self.current = choice.0;
                    self.error = None;
                }
            }
            MetadataMessage::Key(i, key) => {
                if let Some(entry) = self.entries_mut().get_mut(i) {
                    entry.0 = key;
                }
            }
            MetadataMessage::Value(i, value) => {
                if let Some(entry) = self.entries_mut().get_mut(i) {
                    entry.1 = value;
                }
            }
            MetadataMessage::Remove(i) => {
                if i < self.entries_mut().len() {
                    self.entries_mut().remove(i);
                }
            }
            MetadataMessage::Add => self.entries_mut().push((String::new(), String::new())),
            MetadataMessage::Cancel => {}
            MetadataMessage::Apply => {
                let mut commands = Vec::new();
                for scope in &self.scopes {
                    if scope.entries == scope.original {
                        continue;
                    }
                    let mut entries: Vec<KeyValue> = scope
                        .entries
                        .iter()
                        // Rows left completely empty are ignored.
                        .filter(|(k, v)| !(k.trim().is_empty() && v.is_empty()))
                        .map(|(k, v)| KeyValue {
                            key: k.trim().to_owned(),
                            value: (!v.is_empty()).then(|| v.clone()),
                        })
                        .collect();
                    match &scope.column {
                        None => {
                            if entries.iter().any(|kv| kv.key == ARROW_SCHEMA_KEY) {
                                self.error =
                                    Some(format!("{ARROW_SCHEMA_KEY} is managed automatically."));
                                return None;
                            }
                            entries.extend(self.arrow_schema.clone());
                            commands.push(Command::ReplaceMetadata(entries));
                        }
                        Some(column) => commands.push(Command::ReplaceColumnMetadata {
                            column: column.clone(),
                            entries,
                        }),
                    }
                }
                return Some(commands);
            }
        }
        None
    }

    /// Shows an error from applying the command.
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, MetadataMessage> {
        let scope = &self.scopes[self.current];
        let mut rows =
            column![row![text("Key").width(220), text("Value").width(Length::Fill)].spacing(8)]
                .spacing(6);
        for (i, (key, value)) in scope.entries.iter().enumerate() {
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
        if scope.entries.is_empty() {
            rows = rows.push(text("No metadata.").color(tokens.muted_text));
        }
        let choices = self.choices();
        let selected = choices.get(self.current).cloned();
        let mut body = column![
            text("Metadata").size(20),
            row![
                pick_list(choices, selected, MetadataMessage::Scope).width(300),
                text(match scope.column {
                    None => "Key/value pairs stored in the file's footer.",
                    Some(_) => "Key/value pairs stored with the column in the schema.",
                })
                .color(tokens.muted_text),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            scrollable(rows).height(Length::Fixed(280.0)),
            button(text("Add entry"))
                .style(button::secondary)
                .on_press(MetadataMessage::Add),
        ]
        .spacing(14);
        if self.arrow_schema.is_some() && scope.column.is_none() {
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
        let mut dialog = MetadataDialog::new(
            &[kv("owner", "me"), kv(ARROW_SCHEMA_KEY, "xyz")],
            vec![("id".into(), vec![kv("unit", "m")])],
        );
        assert_eq!(dialog.scopes[0].entries.len(), 1);
        assert_eq!(
            dialog.update(MetadataMessage::Apply),
            Some(vec![]),
            "no changes"
        );
        dialog.update(MetadataMessage::Add);
        dialog.update(MetadataMessage::Key(1, " team ".into()));
        dialog.update(MetadataMessage::Add); // left empty: ignored
        dialog.update(MetadataMessage::Value(0, String::new()));
        // A column's metadata, in the same apply.
        dialog.update(MetadataMessage::Scope(ScopeChoice(1, String::new())));
        dialog.update(MetadataMessage::Value(0, "km".into()));
        let Some(commands) = dialog.update(MetadataMessage::Apply) else {
            panic!("expected commands");
        };
        let [Command::ReplaceMetadata(entries), column] = &commands[..] else {
            panic!("expected two commands: {commands:?}");
        };
        assert_eq!(
            column,
            &Command::ReplaceColumnMetadata {
                column: "id".into(),
                entries: vec![kv("unit", "km")]
            }
        );
        let entries = entries.clone();
        dialog.update(MetadataMessage::Scope(ScopeChoice(0, String::new())));
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
        let names = ["a", "b", "c"].map(String::from);
        let mut keep = ChooseColumns::new(true, names.clone(), &[]);
        assert_eq!(keep.update(ChooseMessage::Apply), Some(None));
        keep.update(ChooseMessage::All(false));
        keep.update(ChooseMessage::Toggle(0, true));
        assert_eq!(
            keep.update(ChooseMessage::Apply),
            Some(Some(Command::KeepColumns {
                names: vec!["a".into()]
            }))
        );
        let mut remove = ChooseColumns::new(false, names, &["b".into()]);
        remove.update(ChooseMessage::Toggle(2, true));
        assert_eq!(
            remove.update(ChooseMessage::Apply),
            Some(Some(Command::RemoveColumns {
                names: vec!["b".into(), "c".into()]
            }))
        );
    }

    #[test]
    fn compression_levels() {
        let mut dialog = WriterDialog::new(
            WriterSettings::default(),
            vec![("a".into(), DataType::Int64)],
        );
        dialog.update(WriterMessage::Compression(Some(0), Compression::Zstd(None)));
        dialog.update(WriterMessage::Level(Some(0), Level(Some(7))));
        assert_eq!(
            dialog.column_settings(Some(0)).compression,
            Compression::Zstd(Some(7))
        );
        // Gzip takes 0-10: the level carries over; Brotli then keeps it too.
        dialog.update(WriterMessage::Compression(Some(0), Compression::Gzip(None)));
        assert_eq!(
            dialog.column_settings(Some(0)).compression,
            Compression::Gzip(Some(7))
        );
        dialog.update(WriterMessage::Compression(Some(0), Compression::Snappy));
        assert_eq!(
            dialog.column_settings(Some(0)).compression,
            Compression::Snappy
        );
        assert_eq!(Level::choices(Compression::Zstd(None)).len(), 23);
        assert_eq!(Level::choices(Compression::Snappy), vec![Level(None)]);
    }

    #[test]
    fn cast_dialog_other_types_and_separators() {
        let sample: ArrayRef =
            std::sync::Arc::new(veta_core::arrow::array::Float64Array::from(vec![1234.5]));
        let mut dialog = CastDialog::new("n".into(), sample, None);
        assert_eq!(dialog.target, CastTarget::Text);
        assert!(dialog.uses_separators(), "numbers to text use separators");
        dialog.update(CastMessage::Decimal(Separator::Comma));
        dialog.update(CastMessage::Type(CastTarget::Other));
        assert_eq!(dialog.other, "string");
        dialog.update(CastMessage::Other("decimal(10".into()));
        assert_eq!(dialog.update(CastMessage::Apply), None, "not a type yet");
        dialog.update(CastMessage::Other("decimal(10, 2)".into()));
        let cast = dialog.update(CastMessage::Apply).unwrap();
        assert_eq!(cast.to, DataType::Decimal128(10, 2));
        // Editing a step with a type not in the list shows its name.
        let edit = CastDialog::new("n".into(), dialog.sample.clone(), Some(&cast));
        assert_eq!(
            (edit.target, edit.other.as_str()),
            (CastTarget::Other, "decimal(10, 2)")
        );
    }
}
