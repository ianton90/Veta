//! The shape of the data after each step.

use crate::error::invalid;
use std::sync::Arc;

use arrow::datatypes::{Field, Schema, SchemaRef};

use super::compute::Cache;
use super::{MAX_INSERT_ROWS, Step};
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
            .map_err(|_| invalid!(ColumnNotFound, column = column))
    }

    pub(super) fn field(&self, column: &str) -> Result<&Field> {
        Ok(self.schema.field(self.index(column)?))
    }

    /// Checks a full-pass step against this level before computing it.
    pub(super) fn check(&self, step: &Step) -> Result<()> {
        match step {
            Step::Filter(filter) => filter.validate(&self.schema),
            Step::Sort(sort) => sort.validate(&self.schema),
            Step::Fill(fill) => fill.validate(&self.schema),
            Step::ExplodeList { column } => {
                crate::transforms::nested::explode_fields(&self.schema, column).map(|_| ())
            }
            Step::CollectList { keys, columns } => {
                crate::transforms::nested::collect_fields(&self.schema, keys, columns).map(|_| ())
            }
            Step::Cast(cast) => cast.output_fields(&self.schema).map(|_| ()),
            Step::RemoveEmptyColumn { name } => {
                self.index(name)?;
                if self.schema.fields().len() < 2 {
                    return Err(invalid!(LastColumn));
                }
                Ok(())
            }
            _ => Ok(()),
        }
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
                    return Err(invalid!(InsertPastEnd, row = self.num_rows));
                }
                if *count == 0 {
                    return Err(invalid!(NothingToInsert));
                }
                let too_many = || invalid!(InsertTooMany, max = MAX_INSERT_ROWS);
                if *count > MAX_INSERT_ROWS {
                    return Err(too_many());
                }
                let num_rows = self.num_rows.checked_add(*count).ok_or_else(too_many)?;
                // Inserted rows are empty, so every column must allow nulls.
                let fields = fields.into_iter().map(|f| f.with_nullable(true)).collect();
                Ok(with_fields(fields, num_rows))
            }
            Step::DeleteRows { rows } => {
                if rows.is_empty() {
                    return Err(invalid!(NoRowsToDelete));
                }
                let mut previous_end = 0;
                for (i, r) in rows.iter().enumerate() {
                    if r.is_empty() || (i > 0 && r.start <= previous_end) {
                        return Err(invalid!(RowRanges));
                    }
                    if r.end > self.num_rows {
                        return Err(invalid!(RowPastEnd, row = r.end, rows = self.num_rows));
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
                    return Err(invalid!(ColumnPosition, position = at));
                }
                let mut fields = fields;
                fields.insert(*at, Field::new(name, data_type.clone(), true));
                Ok(with_fields(fields, self.num_rows))
            }
            Step::KeepColumns { names } => {
                if names.is_empty() {
                    return Err(invalid!(KeepNoColumns));
                }
                for name in names {
                    self.index(name)?;
                }
                let fields = fields
                    .into_iter()
                    .filter(|f| names.contains(f.name()))
                    .collect();
                Ok(with_fields(fields, self.num_rows))
            }
            Step::RemoveColumns { names } => {
                if names.is_empty() {
                    return Err(invalid!(NoColumnsToRemove));
                }
                for name in names {
                    self.index(name)?;
                }
                let fields: Vec<Field> = fields
                    .into_iter()
                    .filter(|f| !names.contains(f.name()))
                    .collect();
                if fields.is_empty() {
                    return Err(invalid!(LastColumn));
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
            Step::Cast(cast) => Ok(with_fields(
                cast.output_fields(&self.schema)?,
                self.num_rows,
            )),
            Step::Split(split) => Ok(with_fields(
                split.output_fields(&self.schema)?,
                self.num_rows,
            )),
            Step::ExpandStruct { column, all_levels } => Ok(with_fields(
                crate::transforms::nested::expand_all(&self.schema, column, *all_levels)?
                    .1
                    .fields()
                    .iter()
                    .map(|f| f.as_ref().clone())
                    .collect(),
                self.num_rows,
            )),
            Step::CustomColumn(custom) => Ok(with_fields(
                custom.output_fields(&self.schema)?,
                self.num_rows,
            )),
            Step::AddField {
                column,
                path,
                name,
                data_type,
            } => Ok(with_fields(
                crate::transforms::nested::change_field_fields(
                    &self.schema,
                    column,
                    path,
                    name,
                    Some(data_type),
                )?,
                self.num_rows,
            )),
            Step::RemoveField { column, path, name } => Ok(with_fields(
                crate::transforms::nested::change_field_fields(
                    &self.schema,
                    column,
                    path,
                    name,
                    None,
                )?,
                self.num_rows,
            )),
            Step::CombineStruct { columns, name } => Ok(with_fields(
                crate::transforms::nested::combine_fields(&self.schema, columns, name)?,
                self.num_rows,
            )),
            Step::Merge(merge) => Ok(with_fields(
                merge.output_fields(&self.schema)?,
                self.num_rows,
            )),
            Step::Replace(replace) => {
                replace.validate(&self.schema)?;
                Ok(self.clone())
            }
            Step::FormatText { columns, op } => {
                op.validate(&self.schema, columns)?;
                Ok(self.clone())
            }
            Step::Filter(_)
            | Step::Sort(_)
            | Step::Fill(_)
            | Step::RemoveEmptyColumn { .. }
            | Step::ExplodeList { .. }
            | Step::CollectList { .. } => Err(invalid!(ComputeFirst)),
            Step::MoveColumn { name, to } => {
                let index = self.index(name)?;
                if *to >= fields.len() {
                    return Err(invalid!(ColumnPosition, position = to));
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
        return Err(invalid!(ColumnNameEmpty));
    }
    if schema.fields().iter().any(|f| f.name() == name) {
        return Err(invalid!(ColumnExists, column = name));
    }
    Ok(())
}
