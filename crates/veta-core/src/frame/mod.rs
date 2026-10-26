//! [`DataFrame`]: a table and the steps that made it, and [`Column`].
//!
//! ```no_run
//! # fn main() -> veta_core::Result<()> {
//! let df = veta_core::read_parquet("sales.parquet")?;
//! let df = df.drop_columns(["notes"])?.slice(0..1000, ..)?;
//! println!("{} rows", df.num_rows()?);
//! df.write_parquet("first.parquet")?;
//! # Ok(())
//! # }
//! ```

mod cells;
mod column;
mod columns;
mod history;
mod rows;
mod steps;
mod value;
mod write;

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use arrow::array::ArrayRef;
use arrow::datatypes::{DataType, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;

use crate::error::{Result, invalid};
use crate::files::convert::CsvOptions;
use crate::ops::{Columns, KeepColumns, KeepRows};
use crate::plan::store::Table;
use crate::plan::{Context, History, Node, Progress, Rows, Step, Task};
use crate::sources::{Batches, ReadCsv, ReadJson, ReadParquet};

pub use column::{Column, ColumnRef};
pub use columns::{Fill, ReplaceOptions};
pub use rows::{Order, SortOptions};
pub use value::{IntoValue, Null};

/// How to read a Parquet file.
#[derive(Debug, Clone, Default)]
pub struct ReadOptions {
    /// Read data only when it is needed (the frame stays lazy), instead of
    /// the whole file now.
    pub lazy: bool,
    /// Only these columns, in this order; `None` for all.
    pub columns: Option<Vec<String>>,
}

/// A table and the steps that made it. Methods leave the frame as it is and
/// return a new one, which shares the data it didn't change.
///
/// Eager frames run each step when it is added; lazy ones when data is
/// needed.
#[derive(Debug, Clone)]
pub struct DataFrame {
    node: Arc<Node>,
    lazy: bool,
    /// `None` when history is off.
    history: Option<History>,
    /// Row steps store their whole output (eager only).
    in_place: bool,
    progress: Option<Progress>,
}

impl Default for DataFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads a Parquet file into a new frame.
pub fn read_parquet(path: impl AsRef<Path>) -> Result<DataFrame> {
    DataFrame::new().read_parquet(path)
}

/// Like [`read_parquet`], with options (lazy, some columns).
pub fn read_parquet_with(path: impl AsRef<Path>, options: ReadOptions) -> Result<DataFrame> {
    DataFrame::new().read_parquet_with(path, options)
}

/// Reads a CSV file into a new frame, guessing column types.
pub fn read_csv(path: impl AsRef<Path>) -> Result<DataFrame> {
    DataFrame::new().read_csv(path)
}

/// Like [`read_csv`], with options (delimiter, encoding, types, …).
pub fn read_csv_with(path: impl AsRef<Path>, options: CsvOptions) -> Result<DataFrame> {
    DataFrame::new().read_csv_with(path, options)
}

/// Reads a JSON file (one object per line, or an array of objects) into a
/// new frame.
pub fn read_json(path: impl AsRef<Path>) -> Result<DataFrame> {
    DataFrame::new().read_json(path)
}

impl DataFrame {
    /// A frame with no columns and no rows.
    pub fn new() -> Self {
        Self {
            node: Node::empty(),
            lazy: false,
            history: None,
            in_place: false,
            progress: None,
        }
    }

    /// A frame holding `batches`, which have the columns of `schema`.
    pub fn from_batches(schema: SchemaRef, batches: Vec<RecordBatch>) -> Result<Self> {
        if let Some(bad) = batches
            .iter()
            .find(|b| b.schema().fields() != schema.fields())
        {
            return Err(arrow::error::ArrowError::SchemaError(format!(
                "batch schema {:?} does not match {schema:?}",
                bad.schema()
            ))
            .into());
        }
        Self::new().then(Batches(Table::new(schema, batches)?))
    }

    /// What steps are planned with.
    fn context(&self) -> Context {
        Context {
            lazy: self.lazy,
            in_place: self.in_place && !self.lazy,
            progress: self.progress.clone(),
        }
    }

    /// This frame with `step` added.
    fn then(&self, step: impl Step + 'static) -> Result<Self> {
        self.check_not_broken()?;
        let node = self.node.push(Arc::new(step), &self.context())?;
        Ok(self.changed_to(node))
    }

    /// This frame at `node`, after a change it can undo.
    fn changed_to(&self, node: Arc<Node>) -> Self {
        Self {
            node,
            history: self.history.as_ref().map(|h| h.changed(self.node.clone())),
            ..self.clone()
        }
    }

    /// The progress to report `task` to, if any.
    fn reporting(&self, task: Task) -> Option<(&Progress, Task)> {
        self.progress.as_ref().map(|p| (p, task))
    }

    /// Reports the work on this frame and the frames made from it to
    /// `progress`, which can also cancel it.
    pub fn with_progress(&self, progress: &Progress) -> Self {
        Self {
            progress: Some(progress.clone()),
            ..self.clone()
        }
    }

    // Sources.

    /// Replaces the data with a Parquet file's. The steps before stay in
    /// the chain.
    pub fn read_parquet(&self, path: impl AsRef<Path>) -> Result<Self> {
        self.read_parquet_with(path, ReadOptions::default())
    }

    /// Like [`Self::read_parquet`], with options. The frame becomes lazy or
    /// eager as `options.lazy` says.
    pub fn read_parquet_with(&self, path: impl AsRef<Path>, options: ReadOptions) -> Result<Self> {
        let frame = Self {
            lazy: options.lazy,
            ..self.clone()
        };
        frame.then(ReadParquet {
            path: path.as_ref().to_owned(),
            columns: options.columns,
        })
    }

    /// Replaces the data with a CSV file's, guessing column types.
    pub fn read_csv(&self, path: impl AsRef<Path>) -> Result<Self> {
        self.read_csv_with(path, CsvOptions::default())
    }

    /// Like [`Self::read_csv`], with options.
    pub fn read_csv_with(&self, path: impl AsRef<Path>, options: CsvOptions) -> Result<Self> {
        self.then(ReadCsv {
            path: path.as_ref().to_owned(),
            options,
        })
    }

    /// Replaces the data with a JSON file's.
    pub fn read_json(&self, path: impl AsRef<Path>) -> Result<Self> {
        self.then(ReadJson {
            path: path.as_ref().to_owned(),
        })
    }

    // Modes.

    /// This frame, lazy: later steps run when data is needed.
    pub fn lazy(&self) -> Self {
        Self {
            lazy: true,
            ..self.clone()
        }
    }

    /// Whether steps run when data is needed rather than when added.
    pub fn is_lazy(&self) -> bool {
        self.lazy
    }

    /// This frame, eager, with every step run.
    pub fn collect(&self) -> Result<Self> {
        Ok(Self {
            lazy: false,
            ..self.clone()
        })
    }

    // Inspecting.

    /// Rows; an error if a step is broken.
    pub fn num_rows(&self) -> Result<usize> {
        if let Some((_, error)) = self.broken_step() {
            return Err(error);
        }
        self.node.rows()
    }

    /// Columns. With a broken step, those of its input (as are
    /// [`Self::columns`] and [`Self::schema`]).
    pub fn num_columns(&self) -> usize {
        self.node.schema().fields().len()
    }

    /// Column names, in order.
    pub fn columns(&self) -> Vec<String> {
        self.schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    /// Column names with their types, in order.
    pub fn column_types(&self) -> Vec<(String, DataType)> {
        self.schema()
            .fields()
            .iter()
            .map(|f| (f.name().clone(), f.data_type().clone()))
            .collect()
    }

    /// Columns with their types, nullability and metadata.
    pub fn schema(&self) -> SchemaRef {
        self.node.schema().clone()
    }

    fn schema_ref(&self) -> &Schema {
        self.node.schema()
    }

    /// A column, by name or position.
    pub fn column(&self, column: impl ColumnRef) -> Result<Column> {
        let position = column.position(self.schema_ref())?;
        Ok(Column::new(self.clone(), position))
    }

    /// The value in `row` of `column`, as an array of one value.
    pub fn value(&self, row: usize, column: impl ColumnRef) -> Result<ArrayRef> {
        let rows = self.num_rows()?;
        if row >= rows {
            return Err(invalid!(RowPastEnd, row = row, rows = rows));
        }
        let position = column.position(self.schema_ref())?;
        Ok(self.node.read(row..row + 1, &[position])?.column(0).clone())
    }

    /// Rows `range` (up to the last row) of every column.
    pub fn rows(&self, range: Range<usize>) -> Result<RecordBatch> {
        let columns: Vec<usize> = (0..self.num_columns()).collect();
        self.node.read(range, &columns)
    }

    /// The first `n` rows.
    pub fn head(&self, n: usize) -> Result<RecordBatch> {
        self.rows(0..n)
    }

    /// The last `n` rows.
    pub fn tail(&self, n: usize) -> Result<RecordBatch> {
        let rows = self.num_rows()?;
        self.rows(rows.saturating_sub(n)..rows)
    }

    // Rows.

    /// Keeps only `rows` (a range, a list of rows or a mask).
    pub fn keep_rows(&self, rows: impl Into<Rows>) -> Result<Self> {
        self.then(KeepRows {
            rows: rows.into(),
            drop: false,
        })
    }

    /// Drops `rows` (a range, a list of rows or a mask).
    pub fn drop_rows(&self, rows: impl Into<Rows>) -> Result<Self> {
        self.then(KeepRows {
            rows: rows.into(),
            drop: true,
        })
    }

    /// Keeps `rows` of `columns`, like pandas' `iloc`:
    /// `df.slice(10..20, ["a", "b"])`, `df.slice(vec![true, false, true], ..)`.
    pub fn slice(&self, rows: impl Into<Rows>, columns: impl Into<Columns>) -> Result<Self> {
        let frame = match columns.into() {
            Columns::All => self.clone(),
            columns => self.keep_columns(columns)?,
        };
        match rows.into() {
            Rows::All => Ok(frame),
            rows => frame.keep_rows(rows),
        }
    }

    // Columns.

    /// Keeps only `columns` (names, positions or a mask), in the order
    /// given.
    pub fn keep_columns(&self, columns: impl Into<Columns>) -> Result<Self> {
        self.then(KeepColumns {
            columns: columns.into(),
            drop: false,
        })
    }

    /// Drops `columns` (names, positions or a mask).
    pub fn drop_columns(&self, columns: impl Into<Columns>) -> Result<Self> {
        self.then(KeepColumns {
            columns: columns.into(),
            drop: true,
        })
    }
}
