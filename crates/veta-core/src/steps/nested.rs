//! Steps for nested columns: expanding a struct into columns and exploding
//! a list (or map) into rows.

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, AsArray, FixedSizeListArray, LargeListArray, ListArray, MapArray, UInt32Array,
    new_null_array,
};
use arrow::compute::take;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use super::invalid;
use crate::error::Result;

fn column_index(schema: &Schema, name: &str) -> Result<usize> {
    schema
        .index_of(name)
        .map_err(|_| invalid(format!("no column named {name:?}")))
}

/// Fields of a struct column become columns named `<column>.<field>`,
/// replacing it.
pub(super) fn expand_fields(schema: &Schema, column: &str) -> Result<Vec<Field>> {
    let index = column_index(schema, column)?;
    let DataType::Struct(children) = schema.field(index).data_type() else {
        return Err(invalid(format!("{column} is not a struct column")));
    };
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    let new: Vec<Field> = children
        .iter()
        .map(|child| {
            // A null struct makes every field null.
            Field::new(
                format!("{column}.{}", child.name()),
                child.data_type().clone(),
                true,
            )
        })
        .collect();
    for field in &new {
        if fields.iter().any(|f| f.name() == field.name()) {
            return Err(invalid(format!(
                "a column named {:?} already exists",
                field.name()
            )));
        }
    }
    fields.splice(index..=index, new);
    Ok(fields)
}

pub(super) fn expand(
    batch: &RecordBatch,
    column: &str,
    schema: &Arc<Schema>,
) -> Result<RecordBatch> {
    let index = column_index(&batch.schema(), column)?;
    let array = batch.column(index);
    let structs = array.as_struct();
    let children: Vec<ArrayRef> = structs
        .columns()
        .iter()
        .map(|child| -> Result<ArrayRef> {
            // Push the struct's own nulls down into its fields.
            match structs.nulls() {
                Some(nulls) if nulls.null_count() > 0 => {
                    let mask = arrow::array::BooleanArray::new(nulls.inner().clone(), None);
                    let none = new_null_array(child.data_type(), child.len());
                    Ok(arrow::compute::kernels::zip::zip(&mask, child, &none)?)
                }
                _ => Ok(child.clone()),
            }
        })
        .collect::<Result<_>>()?;
    let mut columns = batch.columns().to_vec();
    columns.splice(index..=index, children);
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
}

/// The struct columns expanded in order (the column, then with
/// `all_levels` its struct fields, breadth first) and the schema after.
pub(super) fn expand_all(
    schema: &Schema,
    column: &str,
    all_levels: bool,
) -> Result<(Vec<String>, Schema)> {
    let mut schema = schema.clone();
    let mut order = Vec::new();
    let mut queue = std::collections::VecDeque::from([column.to_owned()]);
    while let Some(name) = queue.pop_front() {
        schema =
            Schema::new_with_metadata(expand_fields(&schema, &name)?, schema.metadata().clone());
        if all_levels {
            let prefix = format!("{name}.");
            queue.extend(
                schema
                    .fields()
                    .iter()
                    .filter(|f| f.name().starts_with(&prefix))
                    .filter(|f| matches!(f.data_type(), DataType::Struct(_)))
                    .map(|f| f.name().clone()),
            );
        }
        order.push(name);
    }
    Ok((order, schema))
}

/// Expands `column` (and, with `all_levels`, nested structs) in `batch`;
/// `schema` is the output schema.
pub(super) fn expand_levels(
    batch: &RecordBatch,
    column: &str,
    all_levels: bool,
    schema: &Arc<Schema>,
) -> Result<RecordBatch> {
    let (order, _) = expand_all(&batch.schema(), column, all_levels)?;
    let mut batch = batch.clone();
    for (i, name) in order.iter().enumerate() {
        let next = if i + 1 == order.len() {
            schema.clone()
        } else {
            Arc::new(Schema::new_with_metadata(
                expand_fields(&batch.schema(), name)?,
                batch.schema().metadata().clone(),
            ))
        };
        batch = expand(&batch, name, &next)?;
    }
    Ok(batch)
}

/// Fields after combining `columns` into a struct column `name`.
pub(super) fn combine_fields(
    schema: &Schema,
    columns: &[String],
    name: &str,
) -> Result<Vec<Field>> {
    if columns.is_empty() {
        return Err(invalid("choose the columns to combine".into()));
    }
    if name.trim().is_empty() {
        return Err(invalid("column name cannot be empty".into()));
    }
    let indices = columns
        .iter()
        .map(|c| column_index(schema, c))
        .collect::<Result<Vec<_>>>()?;
    let prefix = format!("{name}.");
    let mut children: Vec<Field> = Vec::new();
    for (column, &i) in columns.iter().zip(&indices) {
        let field_name = column.strip_prefix(&prefix).unwrap_or(column);
        if children.iter().any(|c| c.name() == field_name) {
            return Err(invalid(format!("{field_name:?} is chosen twice")));
        }
        children.push(schema.field(i).clone().with_name(field_name));
    }
    let at = *indices.iter().min().expect("not empty");
    let mut fields = Vec::new();
    for (i, field) in schema.fields().iter().enumerate() {
        if i == at {
            fields.push(Field::new(
                name,
                DataType::Struct(children.clone().into()),
                true,
            ));
        } else if !indices.contains(&i) {
            if field.name() == name {
                return Err(invalid(format!("a column named {name:?} already exists")));
            }
            fields.push(field.as_ref().clone());
        }
    }
    Ok(fields)
}

/// Combines `columns` of `batch` into a struct column; `schema` is the
/// output schema.
pub(super) fn combine(
    batch: &RecordBatch,
    columns: &[String],
    name: &str,
    schema: &Arc<Schema>,
) -> Result<RecordBatch> {
    let input = batch.schema();
    let indices = columns
        .iter()
        .map(|c| column_index(&input, c))
        .collect::<Result<Vec<_>>>()?;
    let at = schema.index_of(name)?;
    let DataType::Struct(children) = schema.field(at).data_type() else {
        return Err(invalid(format!("{name} is not a struct column")));
    };
    let arrays = indices.iter().map(|&i| batch.column(i).clone()).collect();
    let combined: ArrayRef = Arc::new(arrow::array::StructArray::try_new(
        children.clone(),
        arrays,
        None,
    )?);
    let mut out = Vec::new();
    let first = *indices.iter().min().expect("not empty");
    for (i, column) in batch.columns().iter().enumerate() {
        if i == first {
            out.push(combined.clone());
        } else if !indices.contains(&i) {
            out.push(column.clone());
        }
    }
    Ok(RecordBatch::try_new_with_options(
        schema.clone(),
        out,
        &arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(batch.num_rows())),
    )?)
}

/// The column type after exploding: the list's item type, or the map's
/// entry struct.
pub(super) fn explode_fields(schema: &Schema, column: &str) -> Result<Vec<Field>> {
    let index = column_index(schema, column)?;
    let item = match schema.field(index).data_type() {
        DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
            item.data_type().clone()
        }
        DataType::Map(entries, _) => entries.data_type().clone(),
        _ => return Err(invalid(format!("{column} is not a list or map column"))),
    };
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields[index] = Field::new(column, item, true);
    Ok(fields)
}

/// One row per item of `column`; empty and null lists give one row with a
/// null item.
pub(super) fn explode(
    batch: &RecordBatch,
    column: &str,
    schema: &Arc<Schema>,
) -> Result<RecordBatch> {
    let index = column_index(&batch.schema(), column)?;
    let array = batch.column(index);
    let (values, ranges): (ArrayRef, Vec<Option<std::ops::Range<usize>>>) = {
        let offsets_of = |offsets: &[usize]| -> Vec<Option<std::ops::Range<usize>>> {
            (0..array.len())
                .map(|i| (array.is_valid(i)).then(|| offsets[i]..offsets[i + 1]))
                .collect()
        };
        match array.data_type() {
            DataType::List(_) => {
                let list = array.as_any().downcast_ref::<ListArray>().expect("list");
                let offsets: Vec<usize> =
                    list.value_offsets().iter().map(|&o| o as usize).collect();
                (list.values().clone(), offsets_of(&offsets))
            }
            DataType::LargeList(_) => {
                let list = array
                    .as_any()
                    .downcast_ref::<LargeListArray>()
                    .expect("list");
                let offsets: Vec<usize> =
                    list.value_offsets().iter().map(|&o| o as usize).collect();
                (list.values().clone(), offsets_of(&offsets))
            }
            DataType::FixedSizeList(_, size) => {
                let list = array
                    .as_any()
                    .downcast_ref::<FixedSizeListArray>()
                    .expect("list");
                let size = *size as usize;
                let offsets: Vec<usize> = (0..=array.len()).map(|i| i * size).collect();
                (list.values().clone(), offsets_of(&offsets))
            }
            DataType::Map(..) => {
                let map = array.as_any().downcast_ref::<MapArray>().expect("map");
                let offsets: Vec<usize> = map.value_offsets().iter().map(|&o| o as usize).collect();
                (
                    Arc::new(map.entries().clone()) as ArrayRef,
                    offsets_of(&offsets),
                )
            }
            _ => return Err(invalid(format!("{column} is not a list or map column"))),
        }
    };
    let mut rows: Vec<u32> = Vec::new();
    let mut items: Vec<Option<u32>> = Vec::new();
    for (row, range) in ranges.into_iter().enumerate() {
        match range.filter(|r| !r.is_empty()) {
            Some(range) => {
                for i in range {
                    rows.push(row as u32);
                    items.push(Some(i as u32));
                }
            }
            None => {
                rows.push(row as u32);
                items.push(None);
            }
        }
    }
    let rows = UInt32Array::from(rows);
    let items = UInt32Array::from(items);
    let columns = batch
        .columns()
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if i == index {
                Ok(take(values.as_ref(), &items, None)?)
            } else {
                Ok(take(c.as_ref(), &rows, None)?)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(schema.clone(), columns)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::format_batch;

    fn batch() -> RecordBatch {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("n.parquet");
        veta_testkit::fixtures::nested(&path);
        let doc = crate::Document::open(&path, crate::OpenOptions::default()).unwrap();
        doc.read(0..4).unwrap()
    }

    #[test]
    fn expand_struct() {
        let batch = batch();
        let schema = Arc::new(Schema::new(
            expand_fields(&batch.schema(), "owner").unwrap(),
        ));
        assert_eq!(schema.field(5).name(), "owner.name");
        assert_eq!(schema.field(6).name(), "owner.home");
        let out = expand(&batch, "owner", &schema).unwrap();
        let cells = format_batch(&out).unwrap();
        assert_eq!(cells[0][5].as_deref(), Some("Ana"));
        assert_eq!(cells[0][6].as_deref(), Some("{x: 5, y: 6}"));
        assert_eq!(cells[2][5], None, "a null struct gives null fields");
        assert!(expand_fields(&batch.schema(), "tags").is_err());
    }

    #[test]
    fn explode_lists_and_maps() {
        let batch = batch();
        let schema = Arc::new(Schema::new(
            explode_fields(&batch.schema(), "tags").unwrap(),
        ));
        let out = explode(&batch, "tags", &schema).unwrap();
        let cells = format_batch(&out).unwrap();
        let tags: Vec<_> = cells.iter().map(|r| (r[0].clone(), r[2].clone())).collect();
        let s = |id: &str, tag: Option<&str>| (Some(id.to_owned()), tag.map(str::to_owned));
        assert_eq!(
            tags,
            [
                s("0", Some("a")),
                s("0", Some("b")),
                s("1", None),
                s("2", None),
                s("3", Some("c")),
                s("3", None),
                s("3", Some("d")),
            ]
        );
        let schema = Arc::new(Schema::new(
            explode_fields(&batch.schema(), "scores").unwrap(),
        ));
        let out = explode(&batch, "scores", &schema).unwrap();
        let cells = format_batch(&out).unwrap();
        assert_eq!(cells[0][4].as_deref(), Some("{keys: math, values: 9}"));
        assert_eq!(out.num_rows(), 5);
        assert!(explode_fields(&batch.schema(), "point").is_err());
    }
}
