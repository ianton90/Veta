//! Nodes: one step each, pointing to the node before it. A frame points to
//! its last node, so adding a step shares everything before it, and earlier
//! frames (and the history) keep their own nodes with their outputs.

use std::ops::Range;
use std::sync::Arc;

use arrow::array::UInt32Array;
use arrow::compute::{concat_batches, take_record_batch};
use arrow::datatypes::Field;
use arrow::datatypes::{Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::progress::{Progress, Task};
use super::step::{Context, Effect, FileSettings, Shape, Step};
use crate::error::Result;
use crate::source::DataSource;
use crate::source::MemorySource;
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
    pub(crate) shape: Shape,
    effect: Effect,
    pub(crate) file: Arc<FileSettings>,
}

impl Node {
    /// No columns, no rows: where every frame starts.
    pub(crate) fn empty() -> Arc<Self> {
        let schema = Arc::new(Schema::empty());
        Arc::new(Self {
            parent: None,
            step: None,
            shape: Shape {
                schema: schema.clone(),
                rows: 0,
            },
            effect: Effect::Data(Arc::new(MemorySource::empty())),
            file: Arc::default(),
        })
    }

    /// This node with `step` after it.
    pub(crate) fn push(
        self: &Arc<Self>,
        step: Arc<dyn Step>,
        context: &Context,
    ) -> Result<Arc<Self>> {
        let planned = step.plan(&self.shape, context)?;
        let rows = match &planned.effect {
            Effect::Data(source) => source.num_rows(),
            Effect::Columns(_) | Effect::Cells(_) | Effect::Patch(_) => self.shape.rows,
            Effect::Rows(selection) => selection.len(),
        };
        Ok(Arc::new(Self {
            parent: Some(self.clone()),
            step: Some(step),
            shape: Shape {
                schema: planned.schema,
                rows,
            },
            effect: planned.effect,
            file: match planned.file {
                Some(file) => Arc::new(file),
                None => self.file.clone(),
            },
        }))
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

    /// How the output is read.
    pub(crate) fn effect(&self) -> &Effect {
        &self.effect
    }

    /// Rows `range` (clamped to the rows there are) of `columns` (output
    /// positions, in the order wanted).
    pub(crate) fn read(&self, range: Range<usize>, columns: &[usize]) -> Result<RecordBatch> {
        let end = range.end.min(self.shape.rows);
        let range = range.start.min(end)..end;
        let schema: SchemaRef = Arc::new(self.shape.schema.project(columns)?);
        let arrays = match &self.effect {
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
                let input_schema = Arc::new(self.parent().shape.schema.project(columns)?);
                concat_batches(&input_schema, &parts)?.columns().to_vec()
            }
        };
        let options = RecordBatchOptions::new().with_row_count(Some(range.len()));
        Ok(RecordBatch::try_new_with_options(schema, arrays, &options)?)
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
    /// first row of each.
    /// The rows done are counted in `progress`, which can cancel the read.
    pub(crate) fn read_parallel<T: Send + 'static>(
        self: &Arc<Self>,
        columns: Option<Vec<usize>>,
        progress: Option<(&Progress, Task)>,
        map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        let node = self.clone();
        let rows = self.shape.rows;
        let progress = progress.map(|(progress, task)| {
            progress.start(task, rows as u64);
            progress.clone()
        });
        let columns = columns.unwrap_or_else(|| (0..self.shape.schema.fields().len()).collect());
        crate::parallel::ordered(
            rows.div_ceil(UNIT_ROWS),
            crate::parallel::threads(),
            move |piece| {
                let first = piece * UNIT_ROWS;
                let end = (first + UNIT_ROWS).min(rows);
                let batches = (first..end)
                    .step_by(PASS_ROWS)
                    .map(|start| {
                        let window = start..(start + PASS_ROWS).min(end);
                        if let Some(progress) = &progress {
                            progress.advance(window.len() as u64)?;
                        }
                        node.read(window, &columns)
                    })
                    .collect::<Result<Vec<_>>>()?;
                map(batches, first)
            },
        )
    }
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
