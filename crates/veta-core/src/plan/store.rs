//! Stored tables: data a step keeps (a file read into memory, a step's
//! output). Tables stay in memory while all of them fit the memory budget;
//! past it, the least recently used are written to Arrow IPC temp files
//! and read back from there. Those files may take a share of the free disk
//! ([`crate::memory::disk_budget`]); past it, the least recently used
//! tables that can be rebuilt (from the file they were read from, or the
//! steps that made them) are deleted, and rebuilt when read again.

use std::fs::File;
use std::io::BufReader;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use arrow::compute::concat_batches;
use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;

use crate::error::Result;
use crate::source::{DataSource, SourceMode, project};
use crate::steps::checkpoint::TempFile;

/// Makes a table's batches again.
pub(crate) type Rebuild = Box<dyn Fn() -> Result<Vec<RecordBatch>> + Send + Sync>;

/// A table kept by a step.
pub(crate) struct Table {
    schema: SchemaRef,
    /// Row at which each batch starts, plus the total.
    offsets: Vec<usize>,
    /// Decoded size in memory.
    bytes: usize,
    state: Mutex<State>,
    /// When last read (a tick of [`CLOCK`]).
    used: AtomicU64,
    rebuild: Option<Rebuild>,
}

impl std::fmt::Debug for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Table")
            .field("schema", &self.schema)
            .field("rows", &self.num_rows())
            .field("state", &*lock(&self.state))
            .finish()
    }
}

#[derive(Debug)]
enum State {
    Memory(Vec<RecordBatch>),
    /// Written to an IPC file of `size` bytes, batch by batch, with its
    /// open reader.
    Disk {
        _file: TempFile,
        size: u64,
        reader: Box<FileReader<BufReader<File>>>,
    },
    /// Deleted from disk, to be rebuilt.
    Gone,
}

/// Ticks on every read, to find the least recently used table.
static CLOCK: AtomicU64 = AtomicU64::new(0);
/// Bytes of the tables held in memory.
static IN_MEMORY: AtomicUsize = AtomicUsize::new(0);
/// Bytes of the tables' files on disk.
static ON_DISK: AtomicU64 = AtomicU64::new(0);
/// Every table still alive.
static TABLES: Mutex<Vec<Weak<Table>>> = Mutex::new(Vec::new());

impl Table {
    /// Keeps `batches` (with the columns of `schema`), writing tables to
    /// disk if memory is over the budget.
    pub(crate) fn new(schema: SchemaRef, batches: Vec<RecordBatch>) -> Result<Arc<Self>> {
        Self::with_rebuild(schema, batches, None)
    }

    /// Like [`Self::new`]; `rebuild` makes the batches again, so the table
    /// can be deleted from disk when the disk budget is short.
    pub(crate) fn with_rebuild(
        schema: SchemaRef,
        batches: Vec<RecordBatch>,
        rebuild: Option<Rebuild>,
    ) -> Result<Arc<Self>> {
        let batches: Vec<_> = batches.into_iter().filter(|b| b.num_rows() > 0).collect();
        let mut offsets = Vec::with_capacity(batches.len() + 1);
        let mut total = 0;
        for batch in &batches {
            offsets.push(total);
            total += batch.num_rows();
        }
        offsets.push(total);
        let bytes = batches.iter().map(RecordBatch::get_array_memory_size).sum();
        let table = Arc::new(Self {
            schema,
            offsets,
            bytes,
            state: Mutex::new(State::Memory(batches)),
            used: AtomicU64::new(CLOCK.fetch_add(1, Ordering::Relaxed)),
            rebuild,
        });
        IN_MEMORY.fetch_add(bytes, Ordering::Relaxed);
        lock(&TABLES).push(Arc::downgrade(&table));
        spill_over(crate::memory::budget())?;
        Ok(table)
    }

    /// Whether the table is on disk.
    #[cfg(test)]
    fn on_disk(&self) -> bool {
        matches!(*lock(&self.state), State::Disk { .. })
    }

    /// Writes the table to an IPC temp file and frees its memory.
    fn spill(&self) -> Result<()> {
        let mut state = lock(&self.state);
        let State::Memory(batches) = &*state else {
            return Ok(());
        };
        let file = TempFile::with_extension("arrow")?;
        let mut writer = FileWriter::try_new_buffered(
            std::fs::OpenOptions::new().write(true).open(file.path())?,
            &self.schema,
        )?;
        for batch in batches {
            writer.write(batch)?;
        }
        writer.finish()?;
        drop(writer);
        let handle = File::open(file.path())?;
        let size = handle.metadata()?.len();
        let reader = Box::new(FileReader::try_new_buffered(handle, None)?);
        *state = State::Disk {
            _file: file,
            size,
            reader,
        };
        IN_MEMORY.fetch_sub(self.bytes, Ordering::Relaxed);
        ON_DISK.fetch_add(size, Ordering::Relaxed);
        Ok(())
    }

    /// Deletes the table's file, if it can be rebuilt.
    fn delete(&self) {
        let mut state = lock(&self.state);
        if let (State::Disk { size, .. }, Some(_)) = (&*state, &self.rebuild) {
            ON_DISK.fetch_sub(*size, Ordering::Relaxed);
            *state = State::Gone;
        }
    }

    /// Makes the batches again after [`Self::delete`], in the same pieces.
    fn restore(&self) -> Result<()> {
        if !matches!(*lock(&self.state), State::Gone) {
            return Ok(());
        }
        let Some(rebuild) = &self.rebuild else {
            return Ok(());
        };
        let all = concat_batches(&self.schema, &rebuild()?)?;
        let batches = self
            .offsets
            .windows(2)
            .map(|w| all.slice(w[0], w[1] - w[0]))
            .collect();
        let mut state = lock(&self.state);
        if matches!(*state, State::Gone) {
            *state = State::Memory(batches);
            IN_MEMORY.fetch_add(self.bytes, Ordering::Relaxed);
        }
        drop(state);
        spill_over(crate::memory::budget())
    }

    /// Batches `first..` that cover `range`, sliced to it.
    fn batches(&self, range: Range<usize>) -> Result<Vec<RecordBatch>> {
        self.used
            .store(CLOCK.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
        let first = self
            .offsets
            .partition_point(|&o| o <= range.start)
            .saturating_sub(1);
        self.restore()?;
        let mut state = lock(&self.state);
        let mut parts = Vec::new();
        for i in first..self.offsets.len() - 1 {
            let (start, end) = (self.offsets[i], self.offsets[i + 1]);
            if start >= range.end {
                break;
            }
            let batch = match &mut *state {
                State::Memory(batches) => batches[i].clone(),
                // Deleted again since restored: read it from scratch.
                State::Gone => {
                    drop(state);
                    return self.batches(range);
                }
                State::Disk { reader, .. } => {
                    reader.set_index(i)?;
                    reader.next().unwrap_or_else(|| {
                        Err(arrow::error::ArrowError::IpcError(format!(
                            "batch {i} missing"
                        )))
                    })?
                }
            };
            let from = range.start.max(start) - start;
            let to = range.end.min(end) - start;
            parts.push(batch.slice(from, to - from));
        }
        Ok(parts)
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        match self.state.get_mut().map(|s| &*s) {
            Ok(State::Memory(_)) => {
                IN_MEMORY.fetch_sub(self.bytes, Ordering::Relaxed);
            }
            Ok(State::Disk { size, .. }) => {
                ON_DISK.fetch_sub(*size, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

impl DataSource for Table {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0)
    }

    fn mode(&self) -> SourceMode {
        SourceMode::InMemory
    }

    fn read(&self, range: Range<usize>) -> Result<RecordBatch> {
        let end = range.end.min(self.num_rows());
        let range = range.start.min(end)..end;
        Ok(concat_batches(&self.schema, &self.batches(range)?)?)
    }

    fn read_columns(&self, range: Range<usize>, names: &[String]) -> Result<RecordBatch> {
        project(&self.read(range)?, names)
    }
}

/// Writes the least recently used tables to disk until those in memory fit
/// `budget` bytes.
fn spill_over(budget: usize) -> Result<()> {
    let mut spilled = false;
    while IN_MEMORY.load(Ordering::Relaxed) > budget {
        let oldest = {
            let mut tables = lock(&TABLES);
            tables.retain(|t| t.strong_count() > 0);
            tables
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|t| matches!(*lock(&t.state), State::Memory(_)))
                .min_by_key(|t| t.used.load(Ordering::Relaxed))
        };
        match oldest {
            Some(table) => table.spill()?,
            None => break,
        }
        spilled = true;
    }
    if spilled && let Some(limit) = crate::memory::disk_budget(ON_DISK.load(Ordering::Relaxed)) {
        delete_over(limit);
    }
    Ok(())
}

/// Deletes the least recently used files of tables that can be rebuilt
/// until the files on disk fit `limit` bytes.
fn delete_over(limit: u64) {
    while ON_DISK.load(Ordering::Relaxed) > limit {
        let oldest = {
            let tables = lock(&TABLES);
            tables
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|t| t.rebuild.is_some() && matches!(*lock(&t.state), State::Disk { .. }))
                .min_by_key(|t| t.used.load(Ordering::Relaxed))
        };
        match oldest {
            Some(table) => table.delete(),
            None => break,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, Int64Array};
    use arrow::datatypes::{DataType, Field, Schema};

    fn table(values: Range<i64>) -> Arc<Table> {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)]));
        let batches = values
            .clone()
            .step_by(1000)
            .map(|start| {
                let column = Int64Array::from_iter_values(start..(start + 1000).min(values.end));
                RecordBatch::try_new(schema.clone(), vec![Arc::new(column)]).unwrap()
            })
            .collect();
        Table::new(schema, batches).unwrap()
    }

    #[test]
    fn deleted_tables_are_rebuilt() {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)]));
        let make = {
            let schema = schema.clone();
            move || {
                let column = Int64Array::from_iter_values(0..5000);
                Ok(vec![RecordBatch::try_new(
                    schema.clone(),
                    vec![Arc::new(column)],
                )?])
            }
        };
        let batches = make().unwrap();
        let rebuilt = Table::with_rebuild(schema, batches, Some(Box::new(make))).unwrap();
        let plain = table(0..5000);
        spill_over(0).unwrap();
        delete_over(0);
        assert!(matches!(*lock(&rebuilt.state), State::Gone));
        assert!(plain.on_disk(), "can't be rebuilt, so it stays");
        let batch = rebuilt.read(4990..5000).unwrap();
        let column = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(column.value(9), 4999);
    }

    #[test]
    fn spilled_tables_read_the_same() {
        let (a, b) = (table(0..10_000), table(10_000..20_000));
        spill_over(0).unwrap();
        assert!(a.on_disk() && b.on_disk());
        for (table, first) in [(&a, 0), (&b, 10_000)] {
            let batch = table.read(1500..2600).unwrap();
            let column = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            assert_eq!(column.len(), 1100);
            assert_eq!(column.value(0), first + 1500);
            assert_eq!(column.value(1099), first + 2599);
        }
    }
}
