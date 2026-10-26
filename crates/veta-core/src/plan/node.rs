//! Nodes: one step each, pointing to the node before it. A frame points to
//! its last node, so adding a step shares everything before it, and earlier
//! frames (and the history) keep their own nodes with their outputs.

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use arrow::array::{ArrayRef, UInt32Array};
use arrow::compute::{concat_batches, take_record_batch};
use arrow::datatypes::{Field, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::progress::{Progress, Task};
use super::step::{Col, Context, Effect, FileSettings, Step};
use super::store::Table;
use crate::error::{Error, Result, invalid};
use crate::source::{DataSource, MemorySource};
use crate::steps::compute::PASS_ROWS;

/// Rows per piece of a pass read on several threads.
const UNIT_ROWS: usize = 16 * PASS_ROWS;

/// Kept input rows further apart than this are read apart.
const GAP_ROWS: usize = 4 * 1024;

/// A step with its output.
#[derive(Debug)]
pub(crate) struct Node {
    parent: Option<Arc<Node>>,
    step: Option<Arc<dyn Step>>,
    schema: SchemaRef,
    effect: Effect,
    /// What an [`Effect::Later`] step computed (or its error).
    computed: OnceLock<Effect>,
    /// The new columns of an [`Effect::Derived`] step, when kept.
    derived: OnceLock<Arc<Table>>,
    rows: OnceLock<usize>,
    pub(crate) file: Arc<FileSettings>,
}

impl Node {
    fn new(
        parent: Option<Arc<Node>>,
        step: Option<Arc<dyn Step>>,
        schema: SchemaRef,
        effect: Effect,
        file: Arc<FileSettings>,
    ) -> Self {
        Self {
            parent,
            step,
            schema,
            effect,
            computed: OnceLock::new(),
            derived: OnceLock::new(),
            rows: OnceLock::new(),
            file,
        }
    }

    /// No columns, no rows: where every frame starts.
    pub(crate) fn empty() -> Arc<Self> {
        let source = Arc::new(MemorySource::empty());
        let schema = source.schema();
        Arc::new(Self::new(
            None,
            None,
            schema,
            Effect::Data(source),
            Arc::default(),
        ))
    }

    /// This node with `step` after it. Unless the context is lazy, the step
    /// runs now.
    pub(crate) fn push(
        self: &Arc<Self>,
        step: Arc<dyn Step>,
        context: &Context,
    ) -> Result<Arc<Self>> {
        let planned = step.plan(self, context)?;
        let file = match planned.file {
            Some(file) => Arc::new(file),
            None => self.file.clone(),
        };
        let node = Self::new(
            Some(self.clone()),
            Some(step),
            planned.schema,
            planned.effect,
            file,
        );
        let node = Arc::new(node);
        if !context.lazy {
            node.run(context)?;
        }
        Ok(node)
    }

    /// Runs the step: computes what a later step makes, keeps the columns a
    /// deriving step makes.
    pub(crate) fn run(self: &Arc<Self>, context: &Context) -> Result<()> {
        match &self.effect {
            Effect::Later => {
                if let Effect::Broken(error) = self.computed(context) {
                    return Err(error.clone());
                }
            }
            Effect::Derived(_) => {
                self.keep_derived(context)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// This node with `step` after it, which doesn't fit (`error`), or
    /// comes after one that doesn't (`None`). Its columns are this node's.
    pub(crate) fn push_broken(
        self: &Arc<Self>,
        step: Arc<dyn Step>,
        error: Option<Error>,
    ) -> Arc<Self> {
        let effect = match error {
            Some(error) => Effect::Broken(error),
            None => Effect::Unplanned,
        };
        let schema = self.schema.clone();
        Arc::new(Self::new(
            Some(self.clone()),
            Some(step),
            schema,
            effect,
            self.file.clone(),
        ))
    }

    fn parent(&self) -> &Node {
        // Only the empty node has no parent, and it reads from its data.
        self.parent.as_deref().unwrap_or(self)
    }

    /// The node before this one (`None` for the empty node).
    pub(crate) fn previous(&self) -> Option<&Arc<Node>> {
        self.parent.as_ref()
    }

    /// The step this node added (`None` for the empty node).
    pub(crate) fn step(&self) -> Option<&Arc<dyn Step>> {
        self.step.as_ref()
    }

    /// The nodes from the first step's to this one.
    pub(crate) fn chain(self: &Arc<Self>) -> Vec<Arc<Node>> {
        let mut nodes = Vec::new();
        let mut node = Some(self);
        while let Some(n) = node.filter(|n| n.step.is_some()) {
            nodes.push(n.clone());
            node = n.parent.as_ref();
        }
        nodes.reverse();
        nodes
    }

    /// The empty node this chain starts from.
    pub(crate) fn root(self: &Arc<Self>) -> Arc<Node> {
        let mut node = self;
        while let Some(parent) = &node.parent {
            node = parent;
        }
        node.clone()
    }

    /// How the output is read, as planned.
    pub(crate) fn effect(&self) -> &Effect {
        &self.effect
    }

    pub(crate) fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// What a later step computed, computing it now if needed. Errors come
    /// back as [`Effect::Broken`].
    fn computed(&self, context: &Context) -> &Effect {
        self.computed.get_or_init(|| {
            let (Some(step), Some(parent)) = (&self.step, &self.parent) else {
                return Effect::Unplanned;
            };
            step.compute(parent, context).unwrap_or_else(Effect::Broken)
        })
    }

    /// How the output is read, with a later step's output computed.
    fn resolved(&self) -> &Effect {
        match &self.effect {
            Effect::Later => self.computed(&Context::default()),
            effect => effect,
        }
    }

    /// Rows of the output (a later step's computed first).
    pub(crate) fn rows(&self) -> Result<usize> {
        if let Some(&rows) = self.rows.get() {
            return Ok(rows);
        }
        let rows = match self.resolved() {
            Effect::Data(source) => source.num_rows(),
            Effect::Rows(selection) => selection.len(),
            Effect::Broken(error) => return Err(error.clone()),
            _ => self.parent().rows()?,
        };
        Ok(*self.rows.get_or_init(|| rows))
    }

    /// Computes and keeps the new columns of a step deriving them.
    fn keep_derived(self: &Arc<Self>, context: &Context) -> Result<&Arc<Table>> {
        if let Some(table) = self.derived.get() {
            return Ok(table);
        }
        let (Effect::Derived(map), Some(step), Some(parent)) =
            (&self.effect, &self.step, &self.parent)
        else {
            return Err(invalid!(ComputeFirst));
        };
        let schema = Arc::new(self.schema.project(&new_positions(map))?);
        let progress = context.progress.as_ref().map(|p| (p, Task::Computing));
        let batches = derive_all(parent, step, &schema, progress)?;
        let (parent, step, again) = (parent.clone(), step.clone(), schema.clone());
        let rebuild = move || derive_all(&parent, &step, &again, None);
        let table = Table::with_rebuild(schema, batches, Some(Box::new(rebuild)))?;
        Ok(self.derived.get_or_init(|| table))
    }

    /// Rows `range` (clamped to the rows there are) of `columns` (output
    /// positions, in the order wanted).
    pub(crate) fn read(&self, range: Range<usize>, columns: &[usize]) -> Result<RecordBatch> {
        let end = range.end.min(self.rows()?);
        let range = range.start.min(end)..end;
        let schema: SchemaRef = Arc::new(self.schema.project(columns)?);
        let arrays = match self.resolved() {
            Effect::Broken(error) => return Err(error.clone()),
            Effect::Later => return Err(invalid!(ComputeFirst)),
            Effect::Unplanned => self
                .parent()
                .read(range.clone(), columns)?
                .columns()
                .to_vec(),
            Effect::Data(source) => {
                let names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
                let batch = source.read_columns(range.clone(), &names)?;
                let positions = names
                    .iter()
                    .map(|n| batch.schema().index_of(n))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                batch.project(&positions)?.columns().to_vec()
            }
            Effect::Columns(map) => {
                let input: Vec<usize> = columns.iter().map(|&c| map[c]).collect();
                self.parent()
                    .read(range.clone(), &input)?
                    .columns()
                    .to_vec()
            }
            Effect::Derived(map) => self.read_derived(map, range.clone(), &schema, columns)?,
            Effect::Cells(cells) => {
                let batch = self.parent().read(range.clone(), columns)?;
                cells.patch(&batch, range.start)?
            }
            Effect::Patch(table) => {
                let base = self.parent().read(range.clone(), columns)?;
                let names: Vec<String> = schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .filter(|n| table.schema().index_of(n).is_ok())
                    .collect();
                let patched = table.read_columns(range.clone(), &names)?;
                schema
                    .fields()
                    .iter()
                    .enumerate()
                    .map(
                        |(i, f): (usize, &Arc<Field>)| match patched.column_by_name(f.name()) {
                            Some(column) => column.clone(),
                            None => base.column(i).clone(),
                        },
                    )
                    .collect()
            }
            Effect::Rows(selection) => {
                let runs = selection.runs(range.clone());
                let parts = groups(&runs)
                    .map(|group| self.read_group(group, columns))
                    .collect::<Result<Vec<_>>>()?;
                let input_schema = Arc::new(self.parent().schema.project(columns)?);
                concat_batches(&input_schema, &parts)?.columns().to_vec()
            }
        };
        let options = RecordBatchOptions::new().with_row_count(Some(range.len()));
        Ok(RecordBatch::try_new_with_options(schema, arrays, &options)?)
    }

    /// `columns` of a step deriving columns: input columns from the input,
    /// new ones kept or derived now.
    fn read_derived(
        &self,
        map: &[Col],
        range: Range<usize>,
        schema: &Schema,
        columns: &[usize],
    ) -> Result<Vec<ArrayRef>> {
        let input: Vec<usize> = columns
            .iter()
            .filter_map(|&c| match map[c] {
                Col::Input(i) => Some(i),
                Col::New(_) => None,
            })
            .collect();
        let from_input = self.parent().read(range.clone(), &input)?;
        let wants_new = columns.iter().any(|&c| matches!(map[c], Col::New(_)));
        let new: Vec<ArrayRef> = match (wants_new, self.derived.get(), &self.step) {
            (false, _, _) | (true, None, None) => Vec::new(),
            (true, Some(table), _) => {
                let names: Vec<String> = new_positions(map)
                    .into_iter()
                    .map(|i| self.schema.field(i).name().clone())
                    .collect();
                table
                    .read_columns(range.clone(), &names)?
                    .columns()
                    .to_vec()
            }
            (true, None, Some(step)) => step.derive(self.parent(), range.clone())?,
        };
        let mut from_input = from_input.columns().iter();
        columns
            .iter()
            .zip(schema.fields())
            .map(|(&c, field)| match map[c] {
                Col::Input(_) => Ok(from_input
                    .next()
                    .cloned()
                    .unwrap_or_else(|| arrow::array::new_empty_array(field.data_type()))),
                Col::New(j) => new.get(j).cloned().ok_or_else(|| invalid!(ComputeFirst)),
            })
            .collect()
    }

    /// The input rows of `runs` (close together), read as one span.
    fn read_group(&self, runs: &[Range<usize>], columns: &[usize]) -> Result<RecordBatch> {
        let span = runs[0].start..runs[runs.len() - 1].end;
        let batch = self.parent().read(span.clone(), columns)?;
        if runs.len() == 1 {
            return Ok(batch);
        }
        let indices: UInt32Array = runs
            .iter()
            .flat_map(|run| run.clone())
            .map(|row| (row - span.start) as u32)
            .collect();
        Ok(take_record_batch(&batch, &indices)?)
    }

    /// Every row of `columns` (output positions; `None` for all), read in
    /// pieces on several threads and handed to `map` in order, with the
    /// first row of each. The rows done are counted in `progress`, which
    /// can cancel the read.
    pub(crate) fn read_parallel<T: Send + 'static>(
        self: &Arc<Self>,
        columns: Option<Vec<usize>>,
        progress: Option<(&Progress, Task)>,
        map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        let node = self.clone();
        let columns = columns.unwrap_or_else(|| (0..self.schema.fields().len()).collect());
        in_pieces(
            self.rows(),
            progress,
            move |window| node.read(window, &columns),
            map,
        )
    }
}

/// Positions of the new columns in a deriving step's output, in order.
fn new_positions(map: &[Col]) -> Vec<usize> {
    let mut new: Vec<(usize, usize)> = map
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c {
            Col::New(j) => Some((*j, i)),
            Col::Input(_) => None,
        })
        .collect();
    new.sort_unstable();
    new.into_iter().map(|(_, i)| i).collect()
}

/// The new columns of `step` (with the fields of `schema`) for every row of
/// its input.
fn derive_all(
    input: &Arc<Node>,
    step: &Arc<dyn Step>,
    schema: &SchemaRef,
    progress: Option<(&Progress, Task)>,
) -> Result<Vec<RecordBatch>> {
    let (node, step, schema) = (input.clone(), step.clone(), schema.clone());
    let read = move |window: Range<usize>| {
        let rows = window.len();
        let columns = step.derive(&node, window)?;
        let options = RecordBatchOptions::new().with_row_count(Some(rows));
        Ok(RecordBatch::try_new_with_options(
            schema.clone(),
            columns,
            &options,
        )?)
    };
    Ok(
        in_pieces(input.rows(), progress, read, |batches, _| Ok(batches))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect(),
    )
}

/// `rows` rows read by `read`, window by window, in pieces on several
/// threads, handed to `map` in order with the first row of each.
fn in_pieces<T: Send + 'static>(
    rows: Result<usize>,
    progress: Option<(&Progress, Task)>,
    read: impl Fn(Range<usize>) -> Result<RecordBatch> + Send + Sync + 'static,
    map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
) -> crate::parallel::Ordered<T> {
    let (rows, error) = match rows {
        Ok(rows) => (rows, None),
        Err(e) => (0, Some(e)),
    };
    let progress = progress.map(|(progress, task)| {
        progress.start(task, rows as u64);
        progress.clone()
    });
    let pieces = match error {
        Some(_) => 1,
        None => rows.div_ceil(UNIT_ROWS),
    };
    crate::parallel::ordered(pieces, crate::parallel::threads(), move |piece| {
        if let Some(error) = &error {
            return Err(error.clone());
        }
        let first = piece * UNIT_ROWS;
        let end = (first + UNIT_ROWS).min(rows);
        let batches = (first..end)
            .step_by(PASS_ROWS)
            .map(|start| {
                let window = start..(start + PASS_ROWS).min(end);
                if let Some(progress) = &progress {
                    progress.advance(window.len() as u64)?;
                }
                read(window)
            })
            .collect::<Result<Vec<_>>>()?;
        map(batches, first)
    })
}

impl Drop for Node {
    /// Drops long chains one node at a time, not by recursion.
    fn drop(&mut self) {
        let mut parent = self.parent.take();
        while let Some(node) = parent {
            parent = match Arc::try_unwrap(node) {
                Ok(mut node) => node.parent.take(),
                Err(_) => None,
            };
        }
    }
}

/// `runs` in groups whose rows are close enough to read together.
fn groups(runs: &[Range<usize>]) -> impl Iterator<Item = &[Range<usize>]> {
    runs.chunk_by(|a, b| b.start - a.end <= GAP_ROWS)
}
