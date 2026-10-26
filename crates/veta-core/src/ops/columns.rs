//! Picking, dropping, renaming, moving and inserting columns.

use std::ops::RangeFull;
use std::sync::Arc;

use arrow::array::new_null_array;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use super::derive::ColumnOp;

use crate::error::{Result, invalid};
use crate::plan::{Context, Effect, Node, Planned, Step};

/// Columns given to a method: all of them (`..`), names, positions or a
/// mask with one value per column (`true` picks the column).
#[derive(Debug, Clone)]
pub enum Columns {
    All,
    Names(Vec<String>),
    Positions(Vec<usize>),
    Mask(Vec<bool>),
}

impl From<RangeFull> for Columns {
    fn from(_: RangeFull) -> Self {
        Columns::All
    }
}

impl From<&str> for Columns {
    fn from(name: &str) -> Self {
        Columns::Names(vec![name.to_owned()])
    }
}

impl<const N: usize> From<[&str; N]> for Columns {
    fn from(names: [&str; N]) -> Self {
        Columns::Names(names.iter().map(|n| n.to_string()).collect())
    }
}

impl From<&[&str]> for Columns {
    fn from(names: &[&str]) -> Self {
        Columns::Names(names.iter().map(|n| n.to_string()).collect())
    }
}

impl From<Vec<&str>> for Columns {
    fn from(names: Vec<&str>) -> Self {
        Columns::Names(names.into_iter().map(str::to_owned).collect())
    }
}

impl From<Vec<String>> for Columns {
    fn from(names: Vec<String>) -> Self {
        Columns::Names(names)
    }
}

impl From<usize> for Columns {
    fn from(position: usize) -> Self {
        Columns::Positions(vec![position])
    }
}

impl<const N: usize> From<[usize; N]> for Columns {
    fn from(positions: [usize; N]) -> Self {
        Columns::Positions(positions.to_vec())
    }
}

impl From<Vec<usize>> for Columns {
    fn from(positions: Vec<usize>) -> Self {
        Columns::Positions(positions)
    }
}

impl From<Vec<bool>> for Columns {
    fn from(mask: Vec<bool>) -> Self {
        Columns::Mask(mask)
    }
}

impl Columns {
    /// The positions in `schema` of the columns given, in their order.
    pub(crate) fn positions(&self, schema: &Schema) -> Result<Vec<usize>> {
        let count = schema.fields().len();
        let positions: Vec<usize> = match self {
            Columns::All => (0..count).collect(),
            Columns::Names(names) => names
                .iter()
                .map(|name| position(schema, name.as_str()))
                .collect::<Result<_>>()?,
            Columns::Positions(positions) => {
                if let Some(&p) = positions.iter().find(|&&p| p >= count) {
                    return Err(invalid!(ColumnPosition, position = p));
                }
                positions.clone()
            }
            Columns::Mask(mask) => {
                if mask.len() != count {
                    return Err(invalid!(MaskLength, found = mask.len(), expected = count));
                }
                (0..count).filter(|&i| mask[i]).collect()
            }
        };
        if let Some((i, _)) = positions
            .iter()
            .enumerate()
            .find(|(i, p)| positions[..*i].contains(p))
        {
            return Err(invalid!(
                ColumnTwice,
                column = schema.field(positions[i]).name()
            ));
        }
        Ok(positions)
    }
}

/// The position of column `name` in `schema`.
pub(crate) fn position(schema: &Schema, name: &str) -> Result<usize> {
    schema
        .index_of(name)
        .map_err(|_| invalid!(ColumnNotFound, column = name))
}

/// Keeps `columns` in their order, or drops them (the rest keep theirs).
#[derive(Debug)]
pub(crate) struct KeepColumns {
    pub(crate) columns: Columns,
    pub(crate) drop: bool,
}

impl Step for KeepColumns {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        let mut positions = self.columns.positions(input.schema())?;
        if self.drop {
            let count = input.schema().fields().len();
            positions = (0..count).filter(|p| !positions.contains(p)).collect();
        }
        Ok(Planned {
            schema: Arc::new(input.schema().project(&positions)?),
            effect: Effect::Columns(positions),
            file: None,
        })
    }
}

/// Renames columns.
#[derive(Debug)]
pub(crate) struct Rename(pub(crate) Vec<(String, String)>);

impl Step for Rename {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        let schema = input.schema();
        let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
        for (from, to) in &self.0 {
            if to.trim().is_empty() {
                return Err(invalid!(ColumnNameEmpty));
            }
            let at = position(schema, from)?;
            fields[at] = fields[at].clone().with_name(to);
        }
        for (i, field) in fields.iter().enumerate() {
            if fields[..i].iter().any(|f| f.name() == field.name()) {
                return Err(invalid!(ColumnExists, column = field.name()));
            }
        }
        Ok(Planned {
            schema: Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
            effect: Effect::Columns((0..schema.fields().len()).collect()),
            file: None,
        })
    }
}

/// Puts columns in a new order.
#[derive(Debug)]
pub(crate) enum Reorder {
    /// `columns`, in their order, to position `at` of the output; the rest
    /// keep theirs.
    Move { columns: Columns, at: usize },
    /// Every column, in this order.
    All(Columns),
}

impl Step for Reorder {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        let schema = input.schema();
        let count = schema.fields().len();
        let order = match self {
            Reorder::Move { columns, at } => {
                let moved = columns.positions(schema)?;
                let mut rest: Vec<usize> = (0..count).filter(|p| !moved.contains(p)).collect();
                if *at > rest.len() {
                    return Err(invalid!(ColumnPosition, position = at));
                }
                rest.splice(*at..*at, moved);
                rest
            }
            Reorder::All(columns) => {
                let order = columns.positions(schema)?;
                if let Some(missing) = (0..count).find(|p| !order.contains(p)) {
                    return Err(invalid!(ReorderAll, column = schema.field(missing).name()));
                }
                order
            }
        };
        Ok(Planned {
            schema: Arc::new(schema.project(&order)?),
            effect: Effect::Columns(order),
            file: None,
        })
    }
}

/// A new column of nulls.
#[derive(Debug)]
pub(crate) struct InsertColumn {
    pub(crate) name: String,
    pub(crate) data_type: DataType,
    pub(crate) at: usize,
}

impl ColumnOp for InsertColumn {
    fn reads(&self) -> Vec<String> {
        Vec::new()
    }

    fn fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        if self.name.trim().is_empty() {
            return Err(invalid!(ColumnNameEmpty));
        }
        if schema.index_of(&self.name).is_ok() {
            return Err(invalid!(ColumnExists, column = self.name));
        }
        let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
        let at = self.at.min(fields.len());
        fields.insert(at, Field::new(&self.name, self.data_type.clone(), true));
        Ok(fields)
    }

    fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        let column = new_null_array(&self.data_type, batch.num_rows());
        Ok(RecordBatch::try_new(schema.clone(), vec![column])?)
    }
}
