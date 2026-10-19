//! [`ParallelWriter`]: writes Parquet files encoding columns on all cores.
//!
//! The same files as `parquet::arrow::ArrowWriter` (row groups of at most
//! the properties' row count, the Arrow schema embedded in the metadata),
//! but columns are encoded on worker threads that keep running while the
//! caller produces the next batch; the finished column chunks are appended
//! to each row group in order.

use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_writer::{
    ArrowColumnChunk, ArrowColumnWriter, ArrowLeafColumn, ArrowRowGroupWriterFactory,
    compute_leaves,
};
use parquet::arrow::{ArrowSchemaConverter, add_encoded_arrow_schema_to_metadata};
use parquet::errors::ParquetError;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;

use crate::error::{Error, Result};

/// Batches a worker may have queued before `write` waits for it.
const QUEUED: usize = 2;

/// Writes record batches to a Parquet file, encoding columns in parallel.
pub struct ParallelWriter<W: Write + Send> {
    writer: SerializedFileWriter<W>,
    factory: ArrowRowGroupWriterFactory,
    schema: SchemaRef,
    max_rows: usize,
    workers: Workers,
    /// Worker encoding each leaf column.
    owner: Vec<usize>,
    /// Whether the workers hold column writers for the current row group.
    open: bool,
    rows: usize,
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
        let leaves = parquet_schema.num_columns();
        let writer =
            SerializedFileWriter::new(out, parquet_schema.root_schema_ptr(), Arc::new(props))?;
        let factory = ArrowRowGroupWriterFactory::new(&writer, schema.clone());
        // More workers than cores: columns cost very different amounts to
        // encode, and the system balances threads better than a fixed split.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let count = leaves.min(4 * threads);
        Ok(Self {
            writer,
            factory,
            schema,
            max_rows,
            workers: Workers::spawn(count),
            owner: (0..leaves).map(|leaf| leaf % count.max(1)).collect(),
            open: false,
            rows: 0,
            row_groups: 0,
        })
    }

    /// Writes `batch` (with the writer's columns), starting new row groups
    /// as they fill up. Returns once the columns are queued for encoding.
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        let mut offset = 0;
        while offset < batch.num_rows() {
            if !self.open {
                let writers = self.factory.create_column_writers(self.row_groups)?;
                let mut per_worker = self.workers.split(&self.owner, writers);
                for (worker, writers) in per_worker.drain(..).enumerate() {
                    self.workers.send(worker, Job::Start(writers))?;
                }
                self.open = true;
            }
            let take = (self.max_rows - self.rows).min(batch.num_rows() - offset);
            self.encode(&batch.slice(offset, take))?;
            self.rows += take;
            offset += take;
            if self.rows >= self.max_rows {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Hands `batch`'s leaf columns to their workers.
    fn encode(&mut self, batch: &RecordBatch) -> Result<()> {
        let mut leaves: Vec<ArrowLeafColumn> = Vec::with_capacity(self.owner.len());
        for (field, array) in self.schema.fields().iter().zip(batch.columns()) {
            leaves.extend(compute_leaves(field, array)?);
        }
        let mut per_worker = self.workers.split(&self.owner, leaves);
        for (worker, leaves) in per_worker.drain(..).enumerate() {
            self.workers.send(worker, Job::Write(leaves))?;
        }
        Ok(())
    }

    /// Finishes the current row group, if any, and writes it out.
    fn flush(&mut self) -> Result<()> {
        if !self.open {
            return Ok(());
        }
        self.open = false;
        // Closing a column writer finishes its last pages: also in parallel.
        let replies = (0..self.workers.len())
            .map(|worker| {
                let (reply, receive) = channel();
                self.workers.send(worker, Job::Close(reply))?;
                Ok(receive)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut done: Vec<std::vec::IntoIter<ArrowColumnChunk>> = Vec::new();
        for receive in replies {
            let chunks = receive.recv().map_err(|_| failed())??;
            done.push(chunks.into_iter());
        }
        let mut row_group = self.writer.next_row_group()?;
        for &worker in &self.owner {
            let chunk = done[worker].next().ok_or_else(failed)?;
            chunk.append_to_row_group(&mut row_group)?;
        }
        row_group.close()?;
        self.rows = 0;
        self.row_groups += 1;
        Ok(())
    }

    /// Writes the last row group and the file's footer.
    pub fn close(mut self) -> Result<()> {
        self.flush()?;
        self.writer.close()?;
        Ok(())
    }
}

fn failed() -> Error {
    Error::InvalidCommand("writing a column failed".into())
}

/// Work for an encoding thread.
enum Job {
    /// The column writers of a new row group.
    Start(Vec<ArrowColumnWriter>),
    /// One batch of the worker's columns, in the same order.
    Write(Vec<ArrowLeafColumn>),
    /// Closes the writers and sends back their chunks (or the first error).
    Close(std::sync::mpsc::Sender<parquet::errors::Result<Vec<ArrowColumnChunk>>>),
}

/// Encoding threads, each owning some of the leaf columns.
struct Workers {
    threads: Vec<(SyncSender<Job>, JoinHandle<()>)>,
}

impl Workers {
    fn spawn(count: usize) -> Self {
        let threads = (0..count)
            .map(|_| {
                let (send, receive) = sync_channel(QUEUED);
                (send, std::thread::spawn(move || work(receive)))
            })
            .collect();
        Self { threads }
    }

    fn len(&self) -> usize {
        self.threads.len()
    }

    /// Distributes `items` (one per leaf) to their owners, keeping order.
    fn split<T>(&self, owner: &[usize], items: Vec<T>) -> Vec<Vec<T>> {
        let mut per_worker: Vec<Vec<T>> = (0..self.len()).map(|_| Vec::new()).collect();
        for (item, &worker) in items.into_iter().zip(owner) {
            per_worker[worker].push(item);
        }
        per_worker
    }

    fn send(&self, worker: usize, job: Job) -> Result<()> {
        self.threads[worker].0.send(job).map_err(|_| failed())
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        for (send, thread) in self.threads.drain(..) {
            drop(send);
            let _ = thread.join();
        }
    }
}

/// An encoding thread's loop: runs until its sender is dropped.
fn work(jobs: Receiver<Job>) {
    let mut writers: Vec<ArrowColumnWriter> = Vec::new();
    let mut error: Option<ParquetError> = None;
    for job in jobs {
        match job {
            Job::Start(new) => writers = new,
            Job::Write(leaves) => {
                if error.is_none() {
                    for (writer, leaf) in writers.iter_mut().zip(&leaves) {
                        if let Err(e) = writer.write(leaf) {
                            error = Some(e);
                            break;
                        }
                    }
                }
            }
            Job::Close(reply) => {
                let result = match error.take() {
                    Some(e) => Err(e),
                    None => writers.drain(..).map(ArrowColumnWriter::close).collect(),
                };
                writers.clear();
                let _ = reply.send(result);
            }
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
