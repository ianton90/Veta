//! [`ParallelWriter`]: writes Parquet files encoding columns on all cores.
//!
//! The same files as `parquet::arrow::ArrowWriter` (row groups of at most
//! the properties' row count, the Arrow schema embedded in the metadata),
//! but each row group's rows are gathered and then every column chunk is
//! encoded as a task of its own on a pool of threads. Several row groups
//! are encoded at once, so the threads stay busy even with few columns or
//! one column much slower than the rest; the finished chunks are appended
//! to the file in order.

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

use crate::error::{Error, Result};

/// Writes record batches to a Parquet file, encoding columns in parallel.
pub struct ParallelWriter<W: Write + Send> {
    writer: SerializedFileWriter<W>,
    factory: ArrowRowGroupWriterFactory,
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

impl<W: Write + Send> ParallelWriter<W> {
    pub fn new(out: W, schema: SchemaRef, mut props: WriterProperties) -> Result<Self> {
        let parquet_schema = ArrowSchemaConverter::new()
            .with_coerce_types(props.coerce_types())
            .convert(&schema)?;
        add_encoded_arrow_schema_to_metadata(&schema, &mut props);
        let max_rows = props.max_row_group_row_count().unwrap_or(usize::MAX).max(1);
        let writer =
            SerializedFileWriter::new(out, parquet_schema.root_schema_ptr(), Arc::new(props))?;
        let factory = ArrowRowGroupWriterFactory::new(&writer, schema.clone());
        let threads = crate::parallel::threads();
        Ok(Self {
            writer,
            factory,
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

    /// Queues the gathered row group, if any, one task per leaf column, and
    /// writes out the row groups finished by then.
    fn flush(&mut self) -> Result<()> {
        if self.gathered.is_empty() {
            return Ok(());
        }
        let writers = self.factory.create_column_writers(self.row_groups)?;
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
            let mut row_group = self.writer.next_row_group()?;
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
        self.writer.close()?;
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
    Error::InvalidCommand("writing a column failed".into())
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    #[test]
    fn writes_what_arrow_writer_writes() {
        let dir = veta_testkit::TempDir::new();
        let path = dir.join("p.parquet");
        let nested = veta_testkit::fixtures::deep_nested_schema();
        let deep = dir.join("deep.parquet");
        veta_testkit::fixtures::deep_nested(&deep, 250);
        let input: Vec<RecordBatch> =
            ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&deep).unwrap())
                .unwrap()
                .with_batch_size(70)
                .build()
                .unwrap()
                .map(Result::unwrap)
                .collect();
        // Row groups of 100 rows from batches of 70: groups split batches.
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(100))
            .build();
        let schema = input[0].schema();
        let mut writer =
            ParallelWriter::new(std::fs::File::create(&path).unwrap(), schema.clone(), props)
                .unwrap();
        for batch in &input {
            writer.write(batch).unwrap();
        }
        writer.close().unwrap();
        let builder =
            ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap()).unwrap();
        let groups: Vec<i64> = builder
            .metadata()
            .row_groups()
            .iter()
            .map(|g| g.num_rows())
            .collect();
        assert_eq!(groups, [100, 100, 50]);
        assert_eq!(
            builder.schema().fields(),
            nested.fields(),
            "Arrow schema embedded"
        );
        let output: Vec<RecordBatch> = builder.build().unwrap().map(Result::unwrap).collect();
        let read = arrow::compute::concat_batches(&schema, &output).unwrap();
        let written = arrow::compute::concat_batches(&schema, &input).unwrap();
        assert_eq!(read, written);

        // A plain table and an empty one.
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("s", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
            ],
        )
        .unwrap();
        let mut writer = ParallelWriter::new(
            std::fs::File::create(&path).unwrap(),
            schema.clone(),
            WriterProperties::default(),
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let back: Vec<RecordBatch> =
            ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap())
                .unwrap()
                .build()
                .unwrap()
                .map(Result::unwrap)
                .collect();
        assert_eq!(back, [batch]);
        let writer = ParallelWriter::new(
            std::fs::File::create(&path).unwrap(),
            schema,
            WriterProperties::default(),
        )
        .unwrap();
        writer.close().unwrap();
        let empty =
            ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(empty.metadata().file_metadata().num_rows(), 0);
    }
}
