use std::sync::Arc;

use arrow::datatypes::{Field, Schema, SchemaRef};

use super::{Step, invalid};
use crate::error::Result;

/// Shape of the data after a step.
#[derive(Debug, Clone)]
pub(super) struct Level {
    pub(super) schema: SchemaRef,
    pub(super) num_rows: usize,
}

impl Level {
    pub(super) fn index(&self, column: &str) -> Result<usize> {
        self.schema
            .index_of(column)
            .map_err(|_| invalid(format!("no column named {column:?}")))
    }

    pub(super) fn field(&self, column: &str) -> Result<&Field> {
        Ok(self.schema.field(self.index(column)?))
    }

    /// Validates `step` against this level and returns the level after it.
    pub(super) fn next(&self, step: &Step) -> Result<Level> {
        let fields: Vec<Field> = self
            .schema
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect();
        let with_fields = |fields: Vec<Field>, num_rows: usize| Level {
            schema: Arc::new(Schema::new_with_metadata(
                fields,
                self.schema.metadata().clone(),
            )),
            num_rows,
        };
        match step {
            Step::EditCells(edits) => {
                edits.validate(self)?;
                Ok(self.clone())
            }
            Step::InsertRows { at, count } => {
                if *at > self.num_rows {
                    return Err(invalid(format!(
                        "cannot insert rows after row {}",
                        self.num_rows
                    )));
                }
                if *count == 0 {
                    return Err(invalid("nothing to insert".into()));
                }
                // Inserted rows are empty, so every column must allow nulls.
                let fields = fields.into_iter().map(|f| f.with_nullable(true)).collect();
                Ok(with_fields(fields, self.num_rows + count))
            }
            Step::DeleteRows { rows } => {
                if rows.is_empty() {
                    return Err(invalid("no rows to delete".into()));
                }
                let mut previous_end = 0;
                for (i, r) in rows.iter().enumerate() {
                    if r.is_empty() || (i > 0 && r.start <= previous_end) {
                        return Err(invalid("row ranges must be sorted and disjoint".into()));
                    }
                    if r.end > self.num_rows {
                        return Err(invalid(format!(
                            "row {} is past the last row ({})",
                            r.end, self.num_rows
                        )));
                    }
                    previous_end = r.end;
                }
                let deleted: usize = rows.iter().map(|r| r.len()).sum();
                Ok(Level {
                    schema: self.schema.clone(),
                    num_rows: self.num_rows - deleted,
                })
            }
            Step::AddColumn {
                name,
                data_type,
                at,
            } => {
                check_new_name(&self.schema, name)?;
                if *at > fields.len() {
                    return Err(invalid(format!("column position {at} is out of range")));
                }
                let mut fields = fields;
                fields.insert(*at, Field::new(name, data_type.clone(), true));
                Ok(with_fields(fields, self.num_rows))
            }
            Step::RemoveColumns { names } => {
                if names.is_empty() {
                    return Err(invalid("no columns to remove".into()));
                }
                for name in names {
                    self.index(name)?;
                }
                let fields: Vec<Field> = fields
                    .into_iter()
                    .filter(|f| !names.contains(f.name()))
                    .collect();
                if fields.is_empty() {
                    return Err(invalid("at least one column must remain".into()));
                }
                Ok(with_fields(fields, self.num_rows))
            }
            Step::RenameColumn { from, to } => {
                let index = self.index(from)?;
                if from != to {
                    check_new_name(&self.schema, to)?;
                }
                let mut fields = fields;
                fields[index] = fields[index].clone().with_name(to);
                Ok(with_fields(fields, self.num_rows))
            }
            Step::MoveColumn { name, to } => {
                let index = self.index(name)?;
                if *to >= fields.len() {
                    return Err(invalid(format!("column position {to} is out of range")));
                }
                let mut fields = fields;
                let field = fields.remove(index);
                fields.insert(*to, field);
                Ok(with_fields(fields, self.num_rows))
            }
        }
    }
}

fn check_new_name(schema: &Schema, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(invalid("column name cannot be empty".into()));
    }
    if schema.fields().iter().any(|f| f.name() == name) {
        return Err(invalid(format!("a column named {name:?} already exists")));
    }
    Ok(())
}
