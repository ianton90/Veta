//! The shape of the data after each step.

use crate::error::invalid;

use arrow::datatypes::{Field, SchemaRef};

use super::compute::Cache;
use crate::error::Result;

/// Shape of the data after a step.
#[derive(Debug, Clone)]
pub(crate) struct Level {
    pub(crate) schema: SchemaRef,
    pub(crate) num_rows: usize,
}

impl Level {
    pub(crate) fn index(&self, column: &str) -> Result<usize> {
        self.schema
            .index_of(column)
            .map_err(|_| invalid!(ColumnNotFound, column = column))
    }

    pub(crate) fn field(&self, column: &str) -> Result<&Field> {
        Ok(self.schema.field(self.index(column)?))
    }

    /// The level after a full-pass step, from its computed result.
    pub(super) fn after_pass(&self, cache: &Cache) -> Level {
        match cache {
            Cache::Rows(rows) => Level {
                schema: self.schema.clone(),
                num_rows: rows.total(),
            },
            Cache::Project(schema) | Cache::Checked(schema) => Level {
                schema: schema.clone(),
                num_rows: self.num_rows,
            },
            Cache::Data(source) => Level {
                schema: source.schema(),
                num_rows: source.num_rows(),
            },
        }
    }
}
