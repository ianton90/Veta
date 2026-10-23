//! [`ParallelWriter`]: writes Parquet files encoding columns on all cores.
//!
//! The same files as `parquet::arrow::ArrowWriter` (row groups of at most
//! the properties' row count, the Arrow schema embedded in the metadata),
//! but each row group's rows are gathered and then every column chunk is
//! encoded as a task of its own on a pool of threads. Several row groups
//! are encoded at once, so the threads stay busy even with few columns or
//! one column much slower than the rest; the finished chunks are appended
//! to the file in order.
//!
//! Dictionary encoding is left out of a column whose first row group has
//! more distinct values than its dictionary page holds (#149): the column
//! writer would build the dictionary and then drop it for plain values.

use crate::error::invalid;
use std::collections::VecDeque;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::{
    ArrowColumnChunk, ArrowColumnWriter, ArrowLeafColumn, ArrowRowGroupWriterFactory,
    compute_leaves,
};
use parquet::arrow::{ArrowSchemaConverter, add_encoded_arrow_schema_to_metadata};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::ColumnPath;

use crate::error::{Error, Result};

/// Writes record batches to a Parquet file, encoding columns in parallel.
pub struct ParallelWriter<W: Write + Send> {
    file: File<W>,
    schema: SchemaRef,
    max_rows: usize,
    pool: Pool,
    /// The rows of the row group being gathered, and their size in bytes.
    gathered: Vec<RecordBatch>,
    rows: usize,
    bytes: usize,
    /// Row groups being encoded, oldest first.
    encoding: VecDeque<Encoding>,
    /// Bytes of the rows of the row groups being encoded.
    encoding_bytes: usize,
    /// Most row groups encoded at once, and most bytes of rows they hold
    /// (more than one row group is let through only within this).
    max_groups: usize,
    max_bytes: usize,
    row_groups: usize,
}

impl<W: Write + Send> std::fmt::Debug for ParallelWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelWriter")
            .field("rows", &self.rows)
            .field("row_groups", &self.row_groups)
            .finish_non_exhaustive()
    }
}

/// The file, opened once the first row group is gathered (its rows decide
/// where dictionaries are left out).
enum File<W: Write + Send> {
    Waiting(Option<(W, WriterProperties)>),
    Open {
        writer: SerializedFileWriter<W>,
        factory: ArrowRowGroupWriterFactory,
    },
}

impl<W: Write + Send> ParallelWriter<W> {
    pub fn new(out: W, schema: SchemaRef, mut props: WriterProperties) -> Result<Self> {
        let parquet_schema = ArrowSchemaConverter::new()
            .with_coerce_types(props.coerce_types())
            .convert(&schema)?;
        add_encoded_arrow_schema_to_metadata(&schema, &mut props);
        let max_rows = props.max_row_group_row_count().unwrap_or(usize::MAX).max(1);
        let threads = crate::parallel::threads();
        Ok(Self {
            file: File::Waiting(Some((out, props))),
            schema,
            max_rows,
            pool: Pool::spawn(threads.min(parquet_schema.num_columns()).max(1)),
            gathered: Vec::new(),
            rows: 0,
            bytes: 0,
            encoding: VecDeque::new(),
            encoding_bytes: 0,
            max_groups: threads.max(2),
            max_bytes: crate::memory::budget() / 8,
            row_groups: 0,
        })
    }

    /// Writes `batch` (with the writer's columns), starting new row groups
    /// as they fill up. Returns once full row groups are queued for
    /// encoding.
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        let size = batch.get_array_memory_size();
        let mut offset = 0;
        while offset < batch.num_rows() {
            let take = (self.max_rows - self.rows).min(batch.num_rows() - offset);
            self.gathered.push(batch.slice(offset, take));
            self.bytes += size * take / batch.num_rows();
            self.rows += take;
            offset += take;
            if self.rows >= self.max_rows {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Opens the file, leaving dictionaries out where the gathered rows
    /// would overflow them.
    fn open(&mut self) -> Result<(&mut SerializedFileWriter<W>, &ArrowRowGroupWriterFactory)> {
        if let File::Waiting(waiting) = &mut self.file {
            let (out, props) = waiting.take().ok_or_else(failed)?;
            let props = crate::profile::time("save: choosing dictionaries", || {
                plan_dictionaries(props, &self.schema, &self.gathered)
            });
            let parquet_schema = ArrowSchemaConverter::new()
                .with_coerce_types(props.coerce_types())
                .convert(&self.schema)?;
            let writer =
                SerializedFileWriter::new(out, parquet_schema.root_schema_ptr(), Arc::new(props))?;
            let factory = ArrowRowGroupWriterFactory::new(&writer, self.schema.clone());
            self.file = File::Open { writer, factory };
        }
        match &mut self.file {
            File::Open { writer, factory } => Ok((writer, factory)),
            File::Waiting(_) => Err(failed()),
        }
    }

    /// Queues the gathered row group, if any, one task per leaf column, and
    /// writes out the row groups finished by then.
    fn flush(&mut self) -> Result<()> {
        if self.gathered.is_empty() {
            return Ok(());
        }
        let row_group = self.row_groups;
        let writers = self.open()?.1.create_column_writers(row_group)?;
        let mut leaves: Vec<Vec<ArrowLeafColumn>> = writers.iter().map(|_| Vec::new()).collect();
        for batch in self.gathered.drain(..) {
            let mut leaf = 0;
            for (field, array) in self.schema.fields().iter().zip(batch.columns()) {
                for column in compute_leaves(field, array)? {
                    leaves[leaf].push(column);
                    leaf += 1;
                }
            }
        }
        let mut chunks = Vec::with_capacity(writers.len());
        for (writer, leaves) in writers.into_iter().zip(leaves) {
            let (reply, receive) = channel();
            self.pool.send(Task {
                writer,
                leaves,
                reply,
            })?;
            chunks.push((receive, None));
        }
        self.encoding.push_back(Encoding {
            bytes: self.bytes,
            chunks,
        });
        self.encoding_bytes += self.bytes;
        self.rows = 0;
        self.bytes = 0;
        self.row_groups += 1;
        self.write_finished(false)
    }

    /// Writes out the finished row groups, oldest first. Waits for the
    /// oldest while too many are being encoded, or for all of them if
    /// `all`.
    fn write_finished(&mut self, all: bool) -> Result<()> {
        loop {
            let wait = all
                || self.encoding.len() > self.max_groups
                || (self.encoding.len() > 1 && self.encoding_bytes > self.max_bytes);
            let Some(oldest) = self.encoding.front_mut() else {
                return Ok(());
            };
            for (receive, chunk) in &mut oldest.chunks {
                if chunk.is_some() {
                    continue;
                }
                let reply = if wait {
                    receive.recv().map_err(|_| failed())?
                } else {
                    match receive.try_recv() {
                        Ok(reply) => reply,
                        Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                        Err(_) => return Err(failed()),
                    }
                };
                *chunk = Some(reply?);
            }
            let Encoding { bytes, chunks } = self.encoding.pop_front().expect("the oldest");
            self.encoding_bytes -= bytes;
            let mut row_group = self.open()?.0.next_row_group()?;
            for (_, chunk) in chunks {
                chunk
                    .ok_or_else(failed)?
                    .append_to_row_group(&mut row_group)?;
            }
            row_group.close()?;
        }
    }

    /// Writes the last row group and the file's footer.
    pub fn close(mut self) -> Result<()> {
        self.flush()?;
        self.write_finished(true)?;
        self.open()?;
        if let File::Open { writer, .. } = self.file {
            writer.close()?;
        }
        Ok(())
    }
}

/// A row group being encoded.
struct Encoding {
    /// Bytes of its rows.
    bytes: usize,
    /// Each leaf column's reply, and its chunk once received.
    chunks: Vec<(Receiver<Reply>, Option<ArrowColumnChunk>)>,
}

/// A column chunk (or the error encoding it).
type Reply = parquet::errors::Result<ArrowColumnChunk>;

fn failed() -> Error {
    invalid!(WriteColumnFailed)
}

/// Encoding one column chunk: the column's writer and its rows.
struct Task {
    writer: ArrowColumnWriter,
    leaves: Vec<ArrowLeafColumn>,
    reply: Sender<Reply>,
}

/// Encoding threads taking tasks from a shared queue, in order.
struct Pool {
    tasks: Option<Sender<Task>>,
    threads: Vec<JoinHandle<()>>,
}

impl Pool {
    fn spawn(count: usize) -> Self {
        let (send, receive) = channel::<Task>();
        let receive = Arc::new(Mutex::new(receive));
        let threads = (0..count)
            .map(|_| {
                let receive = receive.clone();
                std::thread::spawn(move || work(&receive))
            })
            .collect();
        Self {
            tasks: Some(send),
            threads,
        }
    }

    fn send(&self, task: Task) -> Result<()> {
        self.tasks
            .as_ref()
            .and_then(|tasks| tasks.send(task).ok())
            .ok_or_else(failed)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        drop(self.tasks.take());
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Adds its lifetime to a profile part (see [`crate::profile`]).
struct Timer(&'static str, std::time::Instant);

impl Timer {
    fn new(name: &'static str) -> Self {
        Self(name, std::time::Instant::now())
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        crate::profile::add(self.0, self.1.elapsed());
    }
}

/// An encoding thread's loop: runs until the pool's sender is dropped. A
/// task that panics drops its reply, which the writer reports as an error.
fn work(tasks: &Mutex<Receiver<Task>>) {
    loop {
        let task = match tasks.lock() {
            Ok(tasks) => tasks.recv(),
            Err(_) => return,
        };
        let Ok(Task {
            mut writer,
            leaves,
            reply,
        }) = task
        else {
            return;
        };
        let _timer = Timer::new("save: encoding columns (threads)");
        let encoded = std::panic::catch_unwind(AssertUnwindSafe(move || {
            for leaf in &leaves {
                writer.write(leaf)?;
            }
            writer.close()
        }));
        if let Ok(result) = encoded {
            let _ = reply.send(result);
        }
    }
}

/// `props` with the dictionary turned off for each plain top-level column
/// whose distinct values in `rows` would outgrow its dictionary page.
fn plan_dictionaries(
    props: WriterProperties,
    schema: &SchemaRef,
    rows: &[RecordBatch],
) -> WriterProperties {
    let mut off = Vec::new();
    for (i, field) in schema.fields().iter().enumerate() {
        let path = ColumnPath::new(vec![field.name().clone()]);
        if !props.dictionary_enabled(&path) {
            continue;
        }
        let limit = props.column_dictionary_page_size_limit(&path);
        let columns: Vec<&dyn arrow::array::Array> =
            rows.iter().map(|b| b.column(i).as_ref()).collect();
        if dictionary_overflows(&columns, limit) {
            off.push(path);
        }
    }
    if off.is_empty() {
        return props;
    }
    let mut builder = props.into_builder();
    for path in off {
        builder = builder.set_column_dictionary_enabled(path, false);
    }
    builder.build()
}

/// Whether a dictionary of the distinct values in `columns` (one column's
/// pieces) takes more than `limit` bytes. Only plain values (numbers, dates,
/// strings, bytes) are looked at; for other types it says `false`.
fn dictionary_overflows(columns: &[&dyn arrow::array::Array], limit: usize) -> bool {
    use arrow::array::AsArray;
    use arrow::datatypes::DataType;
    // Numbers are read from their values buffer, kept here while `seen`
    // borrows from it.
    let data: Vec<arrow::array::ArrayData> = columns.iter().map(|c| c.to_data()).collect();
    let mut seen: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
    let mut size = 0;
    for (column, data) in columns.iter().zip(&data) {
        // Each distinct value adds its bytes to the dictionary: byte arrays
        // with a 4-byte length, numbers at least 4 bytes (Parquet has no
        // narrower integers).
        let values: Box<dyn Iterator<Item = &[u8]>> = match column.data_type() {
            DataType::Utf8 => Box::new(
                column
                    .as_string::<i32>()
                    .iter()
                    .flatten()
                    .map(str::as_bytes),
            ),
            DataType::LargeUtf8 => Box::new(
                column
                    .as_string::<i64>()
                    .iter()
                    .flatten()
                    .map(str::as_bytes),
            ),
            DataType::Utf8View => {
                Box::new(column.as_string_view().iter().flatten().map(str::as_bytes))
            }
            DataType::Binary => Box::new(column.as_binary::<i32>().iter().flatten()),
            DataType::LargeBinary => Box::new(column.as_binary::<i64>().iter().flatten()),
            DataType::BinaryView => Box::new(column.as_binary_view().iter().flatten()),
            DataType::FixedSizeBinary(_) => {
                Box::new(column.as_fixed_size_binary().iter().flatten())
            }
            t => {
                let Some(width) = t.primitive_width() else {
                    return false;
                };
                let buffer = data.buffers()[0].as_slice();
                let offset = data.offset();
                Box::new(
                    (0..data.len())
                        .filter(|&i| data.is_valid(i))
                        .map(move |i| &buffer[(offset + i) * width..(offset + i + 1) * width]),
                )
            }
        };
        let extra = match column.data_type() {
            t if t.primitive_width().is_some() => 0,
            DataType::FixedSizeBinary(_) => 0,
            _ => 4,
        };
        for value in values {
            if seen.insert(value) {
                size += (value.len() + extra).max(4);
                if size > limit {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests;
