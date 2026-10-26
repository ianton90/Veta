//! Cell edits: new values of single cells, laid over a step's input when it
//! is read. Edits in a row go into one append-only log; each frame sees the
//! log up to its version, so an edit adds one entry (no copy) and undoing
//! one is going back a version. The value before an edit is the input's or
//! an earlier entry's.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use arrow::array::{Array, ArrayRef};
use arrow::compute::interleave;
use arrow::record_batch::RecordBatch;

use crate::error::Result;

/// Bytes counted for an edit besides its value (keys, map nodes).
const EDIT_OVERHEAD: usize = 64;

/// A cell's values by version (the edit's index plus one), oldest first.
type Versions = Vec<(usize, ArrayRef)>;

/// A log of edits, shared by the frames that made it.
#[derive(Debug, Default)]
pub(crate) struct CellLog(RwLock<Entries>);

#[derive(Debug, Default)]
struct Entries {
    /// In the order made: column, row, new value.
    edits: Vec<(Arc<str>, usize, ArrayRef)>,
    /// Bytes of the edits up to each version (`bytes[v]`: the first `v`).
    bytes: Vec<usize>,
    /// By column and row: the versions that set the cell.
    index: HashMap<Arc<str>, BTreeMap<usize, Versions>>,
}

impl Entries {
    fn push(&mut self, column: Arc<str>, row: usize, value: ArrayRef) {
        let total = self.bytes.last().copied().unwrap_or(0);
        if self.bytes.is_empty() {
            self.bytes.push(0);
        }
        self.bytes
            .push(total + value.get_array_memory_size() + EDIT_OVERHEAD);
        let version = self.edits.len() + 1;
        let cells = self.index.entry(column.clone()).or_default();
        cells.entry(row).or_default().push((version, value.clone()));
        self.edits.push((column, row, value));
    }
}

/// The log as one frame sees it: its first `version` edits.
#[derive(Debug, Clone)]
pub(crate) struct CellView {
    log: Arc<CellLog>,
    version: usize,
}

impl CellView {
    /// No edits.
    pub(crate) fn new() -> Self {
        Self {
            log: Arc::default(),
            version: 0,
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Entries> {
        self.log.0.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Entries> {
        self.log.0.write().unwrap_or_else(|p| p.into_inner())
    }

    /// These edits plus `row` of `column` set to `value` (one value, of the
    /// column's type). Added to the log in place when this view is its
    /// latest version; otherwise (after an undo) to a copy of the edits it
    /// sees.
    pub(crate) fn with(&self, column: &str, row: usize, value: ArrayRef) -> Self {
        let column: Arc<str> = Arc::from(column);
        {
            let mut entries = self.write();
            if entries.edits.len() == self.version {
                entries.push(column, row, value);
                return Self {
                    log: self.log.clone(),
                    version: self.version + 1,
                };
            }
        }
        let mut copy = Entries::default();
        for (column, row, value) in &self.read().edits[..self.version] {
            copy.push(column.clone(), *row, value.clone());
        }
        copy.push(column, row, value);
        Self {
            log: Arc::new(CellLog(RwLock::new(copy))),
            version: self.version + 1,
        }
    }

    /// Memory the edits seen take, roughly.
    pub(crate) fn bytes(&self) -> usize {
        self.read().bytes.get(self.version).copied().unwrap_or(0)
    }

    /// The column and row of each edit seen.
    pub(crate) fn cells(&self) -> Vec<(Arc<str>, usize)> {
        self.read().edits[..self.version]
            .iter()
            .map(|(column, row, _)| (column.clone(), *row))
            .collect()
    }

    /// `batch` (rows from `first_row` on) with the edits seen laid over.
    pub(crate) fn patch(&self, batch: &RecordBatch, first_row: usize) -> Result<Vec<ArrayRef>> {
        let rows = first_row..first_row + batch.num_rows();
        let entries = self.read();
        batch
            .schema()
            .fields()
            .iter()
            .zip(batch.columns())
            .map(|(field, column)| {
                let Some(cells) = entries.index.get(field.name().as_str()) else {
                    return Ok(column.clone());
                };
                // Each cell's latest value among the versions seen.
                let edits: Vec<(usize, &ArrayRef)> = cells
                    .range(rows.clone())
                    .filter_map(|(row, versions)| {
                        let seen = versions.partition_point(|(v, _)| *v <= self.version);
                        (seen > 0).then(|| (*row, &versions[seen - 1].1))
                    })
                    .collect();
                if edits.is_empty() {
                    return Ok(column.clone());
                }
                // Each row from the column (source 0) or from its edit.
                let mut sources: Vec<&dyn Array> = vec![column.as_ref()];
                sources.extend(edits.iter().map(|(_, value)| value.as_ref() as &dyn Array));
                let mut indices: Vec<(usize, usize)> = (0..rows.len()).map(|i| (0, i)).collect();
                for (k, (row, _)) in edits.iter().enumerate() {
                    indices[row - first_row] = (k + 1, 0);
                }
                Ok(interleave(&sources, &indices)?)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{AsArray, Int64Array};
    use arrow::datatypes::{DataType, Field, Int64Type, Schema};

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, true)]));
        RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from_iter_values(0..10))]).unwrap()
    }

    fn values(view: &CellView) -> Vec<i64> {
        let patched = view.patch(&batch(), 0).unwrap();
        patched[0].as_primitive::<Int64Type>().values().to_vec()
    }

    fn one(n: i64) -> ArrayRef {
        Arc::new(Int64Array::from(vec![n]))
    }

    #[test]
    fn versions_see_their_own_edits() {
        let a = CellView::new().with("n", 1, one(100));
        let b = a.with("n", 1, one(200)).with("n", 9, one(900));
        assert_eq!(values(&a)[..3], [0, 100, 2]);
        assert_eq!(values(&b)[..3], [0, 200, 2]);
        assert_eq!(values(&b)[9], 900);
        // After going back to `a`, a new edit doesn't touch `b`'s.
        let c = a.with("n", 2, one(300));
        assert_eq!(values(&c)[..3], [0, 100, 300]);
        assert_eq!(values(&b)[..3], [0, 200, 2]);
        assert_eq!(
            (a.cells().len(), b.cells().len(), c.cells().len()),
            (1, 3, 2)
        );
        assert!(b.bytes() > a.bytes());
    }
}
