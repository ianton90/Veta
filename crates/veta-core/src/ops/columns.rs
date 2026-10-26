//! Keeping or dropping columns.

use std::ops::RangeFull;
use std::sync::Arc;

use arrow::datatypes::Schema;

use crate::error::{Result, invalid};
use crate::plan::{Context, Effect, Planned, Shape, Step};

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
    fn positions(&self, schema: &Schema) -> Result<Vec<usize>> {
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
    fn plan(&self, input: &Shape, _context: &Context) -> Result<Planned> {
        let mut positions = self.columns.positions(&input.schema)?;
        if self.drop {
            let count = input.schema.fields().len();
            positions = (0..count).filter(|p| !positions.contains(p)).collect();
        }
        Ok(Planned {
            schema: Arc::new(input.schema.project(&positions)?),
            effect: Effect::Columns(positions),
            file: None,
        })
    }
}
