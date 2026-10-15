//! Keep rows that match conditions.

use super::*;
use iced::widget::column;

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
