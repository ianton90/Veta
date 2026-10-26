//! Nodes: one step each, pointing to the node before it. A frame points to
//! its last node, so adding a step shares everything before it, and earlier
//! frames (and the history) keep their own nodes with their outputs.

use std::ops::Range;
use std::sync::Arc;

use arrow::array::UInt32Array;
use arrow::compute::{concat_batches, take_record_batch};
use arrow::datatypes::{Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};

use super::step::{Effect, FileSettings, Shape, Step};
use crate::error::Result;
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
    #[expect(dead_code, reason = "read by the history (#202)")]
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
    pub(crate) fn push(self: &Arc<Self>, step: Arc<dyn Step>, lazy: bool) -> Result<Arc<Self>> {
        let planned = step.plan(&self.shape, lazy)?;
        let rows = match &planned.effect {
            Effect::Data(source) => source.num_rows(),
            Effect::Columns(_) => self.shape.rows,
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
    pub(crate) fn read_parallel<T: Send + 'static>(
        self: &Arc<Self>,
        columns: Option<Vec<usize>>,
        map: impl Fn(Vec<RecordBatch>, usize) -> Result<T> + Send + Sync + 'static,
    ) -> crate::parallel::Ordered<T> {
        let node = self.clone();
        let rows = self.shape.rows;
        let columns = columns.unwrap_or_else(|| (0..self.shape.schema.fields().len()).collect());
        crate::parallel::ordered(
            rows.div_ceil(UNIT_ROWS),
            crate::parallel::threads(),
            move |piece| {
                let first = piece * UNIT_ROWS;
                let end = (first + UNIT_ROWS).min(rows);
                let batches = (first..end)
                    .step_by(PASS_ROWS)
                    .map(|start| node.read(start..(start + PASS_ROWS).min(end), &columns))
                    .collect::<Result<Vec<_>>>()?;
                map(batches, first)
            },
        )
    }
}

/// `runs` in groups whose rows are close enough to read together.
fn groups(runs: &[Range<usize>]) -> impl Iterator<Item = &[Range<usize>]> {
    runs.chunk_by(|a, b| b.start - a.end <= GAP_ROWS)
}
