//! Column steps: add, remove, keep, rename and move columns, and remove a
//! column if it is empty.

use std::ops::Range;
use std::sync::Arc;

use arrow::array::{Array, new_null_array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use crate::error::{Error, Result, invalid};
use crate::steps::{Cache, Input, Level, Transform, compute_first};
use crate::steps::{ComputeJob, with_schema};

/// [`Step::AddColumn`](crate::steps::Step::AddColumn).
pub(crate) struct AddColumn<'a> {
    pub(crate) name: &'a str,
    pub(crate) data_type: &'a DataType,
    pub(crate) at: usize,
}

impl Transform for AddColumn<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (column != self.name).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        check_new_name(&input.schema, self.name)?;
        let mut fields = input.fields();
        if self.at > fields.len() {
            return Err(invalid!(ColumnPosition, position = self.at));
        }
        fields.insert(self.at, Field::new(self.name, self.data_type.clone(), true));
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        let schema = input.output_schema(names);
        let others: Vec<String> = schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .filter(|n| n != self.name)
            .collect();
        let batch = match names {
            None => input.read(range, None)?,
            Some(_) => input.read(range, Some(&others))?,
        };
        let rows = batch.num_rows();
        let read = batch.schema();
        let columns = schema
            .fields()
            .iter()
            .map(|f| match f.name() == self.name {
                true => Ok(new_null_array(f.data_type(), rows)),
                false => Ok(batch.column(read.index_of(f.name())?).clone()),
            })
            .collect::<Result<Vec<_>>>()?;
        with_schema(schema, columns, rows)
    }
}

/// [`Step::RemoveColumns`](crate::steps::Step::RemoveColumns).
pub(crate) struct RemoveColumns<'a> {
    pub(crate) names: &'a [String],
}

impl Transform for RemoveColumns<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        Some((column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        if self.names.is_empty() {
            return Err(invalid!(NoColumnsToRemove));
        }
        for name in self.names {
            input.index(name)?;
        }
        let fields: Vec<Field> = input
            .fields()
            .into_iter()
            .filter(|f| !self.names.contains(f.name()))
            .collect();
        if fields.is_empty() {
            return Err(invalid!(LastColumn));
        }
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        pick_by_name(input, range, names, str::to_owned)
    }
}

/// [`Step::KeepColumns`](crate::steps::Step::KeepColumns).
pub(crate) struct KeepColumns<'a> {
    pub(crate) names: &'a [String],
}

impl Transform for KeepColumns<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        Some((column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        if self.names.is_empty() {
            return Err(invalid!(KeepNoColumns));
        }
        for name in self.names {
            input.index(name)?;
        }
        let fields = input
            .fields()
            .into_iter()
            .filter(|f| self.names.contains(f.name()))
            .collect();
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        pick_by_name(input, range, names, str::to_owned)
    }
}

/// [`Step::RenameColumn`](crate::steps::Step::RenameColumn).
pub(crate) struct RenameColumn<'a> {
    pub(crate) from: &'a str,
    pub(crate) to: &'a str,
}

impl Transform for RenameColumn<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        let from = if column == self.to { self.from } else { column };
        Some((from.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        let index = input.index(self.from)?;
        if self.from != self.to {
            check_new_name(&input.schema, self.to)?;
        }
        let mut fields = input.fields();
        fields[index] = fields[index].clone().with_name(self.to);
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        // The new name is read under the old one.
        pick_by_name(input, range, names, |name| match name == self.to {
            true => self.from.to_owned(),
            false => name.to_owned(),
        })
    }
}

/// [`Step::MoveColumn`](crate::steps::Step::MoveColumn).
pub(crate) struct MoveColumn<'a> {
    pub(crate) name: &'a str,
    pub(crate) to: usize,
}

impl Transform for MoveColumn<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        Some((column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        let index = input.index(self.name)?;
        let mut fields = input.fields();
        if self.to >= fields.len() {
            return Err(invalid!(ColumnPosition, position = self.to));
        }
        let field = fields.remove(index);
        fields.insert(self.to, field);
        Ok(input.with_fields(fields, input.num_rows))
    }

    fn read(
        &self,
        input: &Input<'_>,
        range: Range<usize>,
        names: Option<&[String]>,
    ) -> Result<RecordBatch> {
        pick_by_name(input, range, names, str::to_owned)
    }
}

/// [`Step::RemoveEmptyColumn`](crate::steps::Step::RemoveEmptyColumn).
pub(crate) struct RemoveEmptyColumn<'a> {
    pub(crate) name: &'a str,
}

impl Transform for RemoveEmptyColumn<'_> {
    fn passes(&self, column: &str) -> Option<(String, u64)> {
        (column != self.name).then(|| (column.to_owned(), 0))
    }

    fn level(&self, input: &Level) -> Result<Level> {
        input.index(self.name)?;
        if input.schema.fields().len() < 2 {
            return Err(invalid!(LastColumn));
        }
        Ok(input.clone())
    }

    fn full_pass(&self) -> bool {
        true
    }

    fn read(&self, _: &Input<'_>, _: Range<usize>, _: Option<&[String]>) -> Result<RecordBatch> {
        compute_first()
    }

    fn compute(
        &self,
        job: &ComputeJob,
        input_rows: usize,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Cache> {
        // Only the column is read, in pieces on all cores.
        let name = self.name;
        let column = name.to_owned();
        let pieces = job.read_parallel(
            input_rows,
            Some(vec![name.to_owned()]),
            move |batches, first| {
                let (mut values, mut example, mut end) = (0, None, first);
                for batch in &batches {
                    let array = batch
                        .column_by_name(&column)
                        .ok_or_else(|| invalid!(ColumnNotFound, column = column))?;
                    values += array.len() - array.null_count();
                    if example.is_none()
                        && let Some(i) = (0..array.len()).find(|&i| array.is_valid(i))
                    {
                        example = arrow::util::display::array_value_to_string(array, i).ok();
                    }
                    end += batch.num_rows();
                }
                Ok((values, example, end))
            },
        );
        let (mut values, mut example) = (0, None);
        for piece in pieces {
            let (count, first, end) = piece?;
            values += count;
            example = example.or(first);
            if !progress(end as f32 / input_rows as f32) {
                return Err(Error::Cancelled);
            }
        }
        if values > 0 {
            return Err(match example {
                Some(example) => invalid!(
                    NotEmptyExample,
                    column = name,
                    values = values,
                    example = example
                ),
                None => invalid!(NotEmpty, column = name, values = values),
            });
        }
        let schema = job.input_schema()?;
        let fields: Vec<_> = schema
            .fields()
            .iter()
            .filter(|f| f.name() != name)
            .map(|f| f.as_ref().clone())
            .collect();
        Ok(Cache::Project(Arc::new(Schema::new_with_metadata(
            fields,
            schema.metadata().clone(),
        ))))
    }
}

/// Reads the output columns `names` (all when `None`) by picking input
/// columns by name; `source` maps an output name to its input name.
fn pick_by_name(
    input: &Input<'_>,
    range: Range<usize>,
    names: Option<&[String]>,
    source: impl Fn(&str) -> String,
) -> Result<RecordBatch> {
    let batch = match names {
        None => input.read(range, None)?,
        Some(_) => {
            let wanted: Vec<String> = input
                .output_schema(names)
                .fields()
                .iter()
                .map(|f| source(f.name()))
                .collect();
            input.read(range, Some(&wanted))?
        }
    };
    input.pick(&batch, names, source)
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
