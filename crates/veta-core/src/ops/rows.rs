//! Row steps: keep or drop rows, sort, reverse, sample, insert, drop
//! nulls, unique, buffer, implode. Each keeps the rows it picks (a
//! selection or an order), or its whole output when it can't.

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{Array, UInt32Array};
use arrow::buffer::BooleanBuffer;
use arrow::compute::concat_batches;
use arrow::datatypes::{Schema, SchemaRef};
use arrow::row::{RowConverter, SortField};

use super::Columns;
use crate::error::{Result, invalid};
use crate::plan::store::Table;
use crate::plan::{Context, Effect, Node, Planned, Rows, Selection, Step, Task};
use crate::steps::MAX_INSERT_ROWS;
use crate::steps::PASS_ROWS;
use crate::transforms::nested::{collect_fields, collect_groups, group_starts};
use crate::transforms::sort::{Sort, SortKey, Sorter, sort_external, sort_in_memory};

/// Every row of `columns` of `input`, as batches.
fn read_columns(
    input: &Arc<Node>,
    columns: Vec<usize>,
    context: &Context,
) -> Result<Vec<arrow::record_batch::RecordBatch>> {
    let progress = context.progress.as_ref().map(|p| (p, Task::Computing));
    Ok(input
        .read_parallel(Some(columns), progress, |batches, _| Ok(batches))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}

/// Keeps `rows`, or all the others when `drop`.
#[derive(Debug)]
pub(crate) struct KeepRows {
    pub(crate) rows: Rows,
    pub(crate) drop: bool,
}

impl KeepRows {
    fn effect(&self, input: &Node) -> Result<Effect> {
        let selection = Selection::new(self.rows.clone(), input.rows()?, self.drop)?;
        Ok(Effect::Rows(Arc::new(selection)))
    }
}

impl Step for KeepRows {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: input.schema().clone(),
            effect: self.effect(input)?,
            file: None,
        })
    }
}

/// Sorts rows by `sort`'s keys (stable).
#[derive(Debug)]
pub(crate) struct SortRows(pub(crate) Sort);

impl Step for SortRows {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        self.0.validate(input.schema())?;
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Later,
            file: None,
        })
    }

    /// When the input is data held in memory, the order of its rows
    /// (4 bytes a row), picked from it as read. Otherwise a sorted copy:
    /// in memory if it fits half the memory budget, else sorted in pieces
    /// on disk.
    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let schema = input.schema().clone();
        let keys = self
            .0
            .keys
            .iter()
            .map(|k| schema.index_of(&k.column))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let in_memory = input.source().is_some_and(|s| s.take(&[], &[]).is_some());
        if in_memory {
            let sorter = Sorter::new(&self.0, &schema)?;
            let batches = read_columns(input, keys, context)?;
            let order = UInt32Array::from(sorter.order(&batches)?);
            return Ok(Effect::Order(Arc::new(order)));
        }
        let rows = input.rows()?;
        let all: Vec<usize> = (0..schema.fields().len()).collect();
        let read = |range: std::ops::Range<usize>| input.read(range, &all);
        let read_keys = |range: std::ops::Range<usize>| input.read(range, &keys);
        if let Some(progress) = &context.progress {
            progress.start(Task::Computing, 0);
        }
        let mut report = |fraction| {
            context
                .progress
                .as_ref()
                .is_none_or(|p| p.set_fraction(fraction))
        };
        let sample = read(0..PASS_ROWS.min(rows))?;
        let bytes =
            sample.get_array_memory_size() as f64 * rows as f64 / sample.num_rows().max(1) as f64;
        if bytes <= crate::memory::budget() as f64 / 2.0 {
            let sorted = sort_in_memory(&self.0, &schema, rows, &read, PASS_ROWS, &mut report)?;
            return Ok(Effect::Data(Table::new(schema, vec![sorted])?));
        }
        let sorted = sort_external(
            &self.0,
            &schema,
            rows,
            &read,
            &read_keys,
            PASS_ROWS,
            None,
            &mut report,
        )?;
        Ok(Effect::Data(Arc::new(sorted)))
    }
}

/// The rows in reverse order.
#[derive(Debug)]
pub(crate) struct Reverse;

impl Reverse {
    fn effect(input: &Node) -> Result<Effect> {
        let rows = input.rows()? as u32;
        Ok(Effect::Order(Arc::new((0..rows).rev().collect())))
    }
}

impl Step for Reverse {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Self::effect(input)?,
            file: None,
        })
    }
}

/// About `fraction` of the rows, picked by their number and `seed`: the
/// same seed picks the same rows of every column, every time.
#[derive(Debug)]
pub(crate) struct Sample {
    pub(crate) fraction: f64,
    pub(crate) seed: u64,
}

impl Sample {
    fn effect(&self, input: &Node) -> Result<Effect> {
        if !(0.0..=1.0).contains(&self.fraction) {
            return Err(invalid!(SampleFraction));
        }
        let limit = (self.fraction * u64::MAX as f64) as u64;
        let picked: BooleanBuffer = (0..input.rows()? as u64)
            .map(|row| mix(row, self.seed) <= limit && self.fraction > 0.0)
            .collect();
        Ok(Effect::Rows(Arc::new(Selection::new(
            Rows::Mask(picked),
            input.rows()?,
            false,
        )?)))
    }
}

/// A well-spread hash of a row number and a seed (SplitMix64).
fn mix(row: u64, seed: u64) -> u64 {
    let mut z = row
        .wrapping_add(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Step for Sample {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: input.schema().clone(),
            effect: self.effect(input)?,
            file: None,
        })
    }
}

/// `count` empty rows at row `at`.
#[derive(Debug)]
pub(crate) struct InsertRows {
    pub(crate) at: usize,
    pub(crate) count: usize,
}

impl InsertRows {
    fn schema(&self, input: &Node) -> Result<SchemaRef> {
        let rows = input.rows()?;
        if self.at > rows {
            return Err(invalid!(InsertPastEnd, row = rows));
        }
        if self.count == 0 {
            return Err(invalid!(NothingToInsert));
        }
        if self.count > MAX_INSERT_ROWS {
            return Err(invalid!(InsertTooMany, max = MAX_INSERT_ROWS));
        }
        // Inserted rows are empty, so every column must allow nulls.
        let fields: Vec<_> = input
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone().with_nullable(true))
            .collect();
        Ok(Arc::new(Schema::new_with_metadata(
            fields,
            input.schema().metadata().clone(),
        )))
    }
}

impl Step for InsertRows {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: self.schema(input)?,
            effect: Effect::Insert {
                at: self.at,
                count: self.count,
            },
            file: None,
        })
    }
}

/// Drops the rows with a null in any of `columns` (all of them when none
/// are given).
#[derive(Debug)]
pub(crate) struct DropNulls(pub(crate) Columns);

impl DropNulls {
    fn positions(&self, schema: &Schema) -> Result<Vec<usize>> {
        let positions = self.0.positions(schema)?;
        Ok(match positions.is_empty() {
            true => (0..schema.fields().len()).collect(),
            false => positions,
        })
    }
}

impl Step for DropNulls {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        self.positions(input.schema())?;
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Later,
            file: None,
        })
    }

    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let batches = read_columns(input, self.positions(input.schema())?, context)?;
        let mut keep = Vec::with_capacity(input.rows()?);
        for batch in &batches {
            keep.extend(
                (0..batch.num_rows()).map(|i| batch.columns().iter().all(|c| c.is_valid(i))),
            );
        }
        let selection =
            Selection::new(Rows::Mask(BooleanBuffer::from(keep)), input.rows()?, false)?;
        Ok(Effect::Rows(Arc::new(selection)))
    }
}

/// Which of rows with equal values [`DropDuplicates`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The first of them.
    First,
    /// The last of them.
    Last,
    /// None of them: only rows whose values appear once stay.
    None,
}

/// Keeps one row of each group of rows equal in `columns` (all of them when
/// none are given), in their order.
#[derive(Debug)]
pub(crate) struct DropDuplicates {
    pub(crate) columns: Columns,
    pub(crate) keep: Keep,
}

impl DropDuplicates {
    fn converter(&self, schema: &Schema) -> Result<(Vec<usize>, RowConverter)> {
        let positions = DropNulls(self.columns.clone()).positions(schema)?;
        let fields: Vec<SortField> = positions
            .iter()
            .map(|&i| SortField::new(schema.field(i).data_type().clone()))
            .collect();
        if !RowConverter::supports_fields(&fields) {
            return Err(invalid!(UniqueUnsupported));
        }
        Ok((positions, RowConverter::new(fields)?))
    }
}

impl Step for DropDuplicates {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        self.converter(input.schema())?;
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Later,
            file: None,
        })
    }

    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let (positions, converter) = self.converter(input.schema())?;
        let batches = read_columns(input, positions, context)?;
        // For each distinct row: its first and last row, and how many.
        let mut seen: HashMap<arrow::row::OwnedRow, (usize, usize, usize)> = HashMap::new();
        let mut row = 0;
        for batch in &batches {
            let rows = converter.convert_columns(batch.columns())?;
            for key in rows.iter() {
                let entry = seen.entry(key.owned()).or_insert((row, row, 0));
                entry.1 = row;
                entry.2 += 1;
                row += 1;
            }
        }
        let kept_rows: Vec<usize> = seen
            .into_values()
            .filter_map(|(first, last, count)| match self.keep {
                Keep::First => Some(first),
                Keep::Last => Some(last),
                Keep::None => (count == 1).then_some(first),
            })
            .collect();
        let selection = Selection::new(Rows::List(kept_rows), input.rows()?, false)?;
        Ok(Effect::Rows(Arc::new(selection)))
    }
}

/// The output so far, kept whole: later steps read from it.
#[derive(Debug)]
pub(crate) struct Buffer;

impl Step for Buffer {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: input.schema().clone(),
            effect: Effect::Later,
            file: None,
        })
    }

    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let all = (0..input.schema().fields().len()).collect();
        Node::kept(input, input.schema().clone(), Effect::Columns(all), context)
    }
}

/// One row per group of equal `keys` (in their order), with `columns`
/// collected into lists; other columns are dropped.
#[derive(Debug)]
pub(crate) struct Implode {
    pub(crate) keys: Vec<String>,
    pub(crate) columns: Vec<String>,
}

impl Implode {
    fn schema(&self, input: &Node) -> Result<SchemaRef> {
        let fields = collect_fields(input.schema(), &self.keys, &self.columns)?;
        Ok(Arc::new(Schema::new_with_metadata(
            fields,
            input.schema().metadata().clone(),
        )))
    }
}

impl Step for Implode {
    fn plan(&self, input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: self.schema(input)?,
            effect: Effect::Later,
            file: None,
        })
    }

    /// Rows ordered by the keys (stable, so items keep their order), then
    /// each run of equal keys becomes a row.
    fn compute(&self, input: &Arc<Node>, context: &Context) -> Result<Effect> {
        let schema = self.schema(input)?;
        let sort = ascending(&self.keys);
        let source = match self.keys.is_empty() {
            true => input.clone(),
            false => Arc::new(Node::over(
                input,
                SortRows(sort.clone()).compute(input, context)?,
            )),
        };
        let used = schema
            .fields()
            .iter()
            .map(|f| source.schema().index_of(f.name()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let read_schema = Arc::new(source.schema().project(&used)?);
        let batch = concat_batches(&read_schema, &read_columns(&source, used, context)?)?;
        let keys = self
            .keys
            .iter()
            .map(|k| read_schema.index_of(k))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let sorter = match self.keys.is_empty() {
            true => None,
            false => Some(Sorter::new(&sort, &read_schema)?),
        };
        let mut bounds = group_starts(&batch, &keys, sorter.as_ref().map(|s| &s.converter))?;
        bounds.push(batch.num_rows());
        let output = collect_groups(&batch, &bounds, &self.keys, &schema)?;
        Ok(Effect::Data(Table::new(schema, vec![output])?))
    }
}

/// Sorting by `keys`, ascending, nulls last.
fn ascending(keys: &[String]) -> Sort {
    Sort {
        keys: keys
            .iter()
            .map(|k| SortKey {
                column: k.clone(),
                descending: false,
                nulls_first: false,
            })
            .collect(),
    }
}
