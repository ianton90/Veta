//! Column methods: rename, move, insert, cast, fill, replace, format,
//! split, merge, nested values, shift.

use arrow::array::ArrayRef;
use arrow::compute::{CastOptions as ArrowCastOptions, cast_with_options};
use arrow::datatypes::DataType;

use super::{DataFrame, IntoValue};
use crate::display::PathPart;
use crate::error::{Result, invalid};
use crate::ops::{
    ChangeField, Columns, Explode, FillNearest, FillValue, FormatText, InsertColumn, Rename,
    Reorder, Shift, Unnest,
};
use crate::transforms::cast::{Cast, CastOptions};
use crate::transforms::split::{Merge, Split, SplitBy};
use crate::transforms::text::{Replace, TextOp};

/// How [`DataFrame::fill_null`] fills nulls.
#[derive(Debug, Clone)]
pub enum Fill {
    /// With this value (one, converted to the column's type).
    Value(ArrayRef),
    /// With the nearest value above.
    Down,
    /// With the nearest value below.
    Up,
}

impl Fill {
    /// With `value`: `Fill::value(0)`, `Fill::value("n/a")`.
    pub fn value(value: impl IntoValue) -> Self {
        Fill::Value(value.into_value())
    }
}

/// How [`DataFrame::replace_with`] matches.
#[derive(Debug, Clone, Copy)]
pub struct ReplaceOptions {
    /// Text columns: only cells equal to the value found, instead of every
    /// occurrence inside cells. Other columns always match whole cells.
    pub whole_cell: bool,
    /// Text columns: the same upper and lower case only.
    pub case_sensitive: bool,
}

impl Default for ReplaceOptions {
    fn default() -> Self {
        Self {
            whole_cell: false,
            case_sensitive: true,
        }
    }
}

/// `value` as text for a step that reads it against a column's type;
/// `None` for null.
fn as_text(value: impl IntoValue) -> Result<Option<String>> {
    let value = value.into_value();
    if value.logical_null_count() == value.len() {
        return Ok(None);
    }
    let text = arrow::compute::cast(&value, &DataType::Utf8)?;
    Ok(Some(
        text.as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .map(|s| s.value(0).to_owned())
            .unwrap_or_default(),
    ))
}

impl DataFrame {
    /// Renames columns: `df.rename([("old", "new")])`.
    pub fn rename<F: Into<String>, T: Into<String>>(
        &self,
        names: impl IntoIterator<Item = (F, T)>,
    ) -> Result<Self> {
        self.then(Rename(
            names
                .into_iter()
                .map(|(f, t)| (f.into(), t.into()))
                .collect(),
        ))
    }

    /// Moves `column` to position `at` (counted without it).
    pub fn move_column(&self, column: &str, at: usize) -> Result<Self> {
        self.move_columns(column, at)
    }

    /// Moves `columns`, in the order given, to position `at` (counted
    /// without them); the rest keep their order.
    pub fn move_columns(&self, columns: impl Into<Columns>, at: usize) -> Result<Self> {
        self.then(Reorder::Move {
            columns: columns.into(),
            at,
        })
    }

    /// Puts every column in the order given.
    pub fn reorder_columns(&self, columns: impl Into<Columns>) -> Result<Self> {
        self.then(Reorder::All(columns.into()))
    }

    /// A new column of nulls at position `at`.
    pub fn insert_column(&self, name: &str, data_type: DataType, at: usize) -> Result<Self> {
        if at > self.num_columns() {
            return Err(invalid!(ColumnPosition, position = at));
        }
        self.then(InsertColumn {
            name: name.to_owned(),
            data_type,
            at,
        })
    }

    /// Converts `column` to `to`, reading text as `options` say
    /// (separators, date format) and doing with values that don't convert
    /// what they say (`CastOptions::default()`: they become null).
    pub fn cast(&self, column: &str, to: DataType, options: CastOptions) -> Result<Self> {
        self.then(Cast {
            column: column.to_owned(),
            to,
            options,
        })
    }

    /// Fills the nulls of `column`: `df.fill_null("a", Fill::Down)`.
    pub fn fill_null(&self, column: &str, with: Fill) -> Result<Self> {
        match with {
            Fill::Value(value) => {
                let field = self
                    .schema_ref()
                    .field(super::ColumnRef::position(&column, self.schema_ref())?)
                    .clone();
                let options = ArrowCastOptions {
                    safe: false,
                    ..Default::default()
                };
                let value = cast_with_options(&value, field.data_type(), &options)
                    .map_err(|_| invalid!(EditType, column = column))?;
                self.then(FillValue {
                    column: column.to_owned(),
                    value,
                })
            }
            Fill::Down | Fill::Up => self.then(FillNearest {
                column: column.to_owned(),
                up: matches!(with, Fill::Up),
            }),
        }
    }

    /// Replaces `find` with `with` in `column`, matching as `options` say
    /// (`ReplaceOptions::default()`: inside text cells too, same case):
    /// `df.replace("note", Null, "n/a", ReplaceOptions::default())`.
    pub fn replace(
        &self,
        column: &str,
        find: impl IntoValue,
        with: impl IntoValue,
        options: ReplaceOptions,
    ) -> Result<Self> {
        let find = as_text(find)?;
        self.then(Replace {
            column: column.to_owned(),
            // Nulls are whole cells.
            whole_cell: options.whole_cell || find.is_none(),
            find,
            with: as_text(with)?,
            case_sensitive: options.case_sensitive,
        })
    }

    /// Trims or changes the case of the text in `columns`.
    pub fn format_text(&self, columns: impl Into<Columns>, op: TextOp) -> Result<Self> {
        let columns = columns.into().positions(self.schema_ref())?;
        self.then(FormatText {
            columns: columns
                .iter()
                .map(|&i| self.schema_ref().field(i).name().clone())
                .collect(),
            op,
        })
    }

    /// Splits the text of `column` into columns `<column>.1`, `<column>.2`,
    /// …, which replace it.
    pub fn split(&self, column: &str, by: SplitBy) -> Result<Self> {
        self.then(Split {
            column: column.to_owned(),
            by,
        })
    }

    /// Joins the text of `columns` with `separator` into column `name`,
    /// where the first of them was; they are removed.
    pub fn merge_columns(
        &self,
        columns: impl Into<Columns>,
        separator: &str,
        name: &str,
    ) -> Result<Self> {
        let columns = columns.into().positions(self.schema_ref())?;
        self.then(Merge {
            columns: columns
                .iter()
                .map(|&i| self.schema_ref().field(i).name().clone())
                .collect(),
            separator: separator.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The fields of struct `column` become columns `<column>.<field>`.
    pub fn unnest(&self, column: &str) -> Result<Self> {
        self.then(Unnest {
            column: column.to_owned(),
            all_levels: false,
        })
    }

    /// Like [`Self::unnest`], with the structs inside it too.
    pub fn unnest_all(&self, column: &str) -> Result<Self> {
        self.then(Unnest {
            column: column.to_owned(),
            all_levels: true,
        })
    }

    /// One row per item of list (or map) `column`; empty and null lists
    /// give one row with a null item.
    pub fn explode(&self, column: &str) -> Result<Self> {
        self.then(Explode {
            column: column.to_owned(),
        })
    }

    /// Adds a null field `name` at the end of the struct at `path` in
    /// `column` (an empty path: the column is the struct).
    pub fn add_field(
        &self,
        column: &str,
        path: &[PathPart],
        name: &str,
        data_type: DataType,
    ) -> Result<Self> {
        self.then(ChangeField {
            column: column.to_owned(),
            path: path.to_vec(),
            name: name.to_owned(),
            add: Some(data_type),
        })
    }

    /// Removes field `name` from the struct at `path` in `column`.
    pub fn remove_field(&self, column: &str, path: &[PathPart], name: &str) -> Result<Self> {
        self.then(ChangeField {
            column: column.to_owned(),
            path: path.to_vec(),
            name: name.to_owned(),
            add: None,
        })
    }

    /// Moves the values of `column` down by `rows` (up when negative); the
    /// rows left empty become null.
    pub fn shift(&self, column: &str, rows: i64) -> Result<Self> {
        self.then(Shift {
            columns: column.into(),
            rows,
        })
    }

    /// Like [`Self::shift`], for every column.
    pub fn shift_all(&self, rows: i64) -> Result<Self> {
        self.then(Shift {
            columns: Columns::All,
            rows,
        })
    }
}
