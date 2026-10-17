//! [`ParallelWriter`]: writes Parquet files encoding columns on all cores.
//!
//! The same files as `parquet::arrow::ArrowWriter` (row groups of at most
//! the properties' row count, the Arrow schema embedded in the metadata),
//! but each batch's columns are encoded at the same time, one column per
//! core, and the finished column chunks appended to the row group in order.

use std::io::Write;
use std::sync::Arc;

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
    /// The row group being written: its column writers and rows so far.
    columns: Option<Vec<ArrowColumnWriter>>,
    rows: usize,
    row_groups: usize,
    threads: usize,
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
        Ok(Self {
            writer,
            factory,
            schema,
            max_rows,
            columns: None,
            rows: 0,
            row_groups: 0,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        })
    }

    /// Writes `batch` (with the writer's columns), starting new row groups
    /// as they fill up.
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        let mut offset = 0;
        while offset < batch.num_rows() {
            if self.columns.is_none() {
                self.columns = Some(self.factory.create_column_writers(self.row_groups)?);
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

    /// Encodes `batch` into the current row group, columns in parallel.
    fn encode(&mut self, batch: &RecordBatch) -> Result<()> {
        let mut leaves: Vec<ArrowLeafColumn> = Vec::new();
        for (field, array) in self.schema.fields().iter().zip(batch.columns()) {
            leaves.extend(compute_leaves(field, array)?);
        }
        let columns = self.columns.as_mut().expect("a row group is open");
        let per_thread = columns.len().div_ceil(self.threads).max(1);
        std::thread::scope(|scope| {
            let work: Vec<_> = columns
                .chunks_mut(per_thread)
                .zip(leaves.chunks(per_thread))
                .map(|(writers, leaves)| {
                    scope.spawn(move || {
                        for (writer, leaf) in writers.iter_mut().zip(leaves) {
                            writer.write(leaf)?;
                        }
                        Ok::<_, parquet::errors::ParquetError>(())
                    })
                })
                .collect();
            work.into_iter().try_for_each(|w| {
                w.join()
                    .map_err(|_| Error::InvalidCommand("writing a column failed".into()))?
                    .map_err(Error::from)
            })
        })
    }

    /// Finishes the current row group, if any, and writes it out.
    fn flush(&mut self) -> Result<()> {
        let Some(columns) = self.columns.take() else {
            return Ok(());
        };
        let per_thread = columns.len().div_ceil(self.threads).max(1);
        let mut columns = columns;
        // Closing a column writer finishes its last pages: also in parallel.
        let chunks: Vec<ArrowColumnChunk> = std::thread::scope(|scope| {
            let mut groups = Vec::new();
            while !columns.is_empty() {
                let rest = columns.split_off(per_thread.min(columns.len()));
                groups.push(std::mem::replace(&mut columns, rest));
            }
            let work: Vec<_> = groups
                .into_iter()
                .map(|group| {
                    scope.spawn(move || {
                        group
                            .into_iter()
                            .map(ArrowColumnWriter::close)
                            .collect::<parquet::errors::Result<Vec<_>>>()
                    })
                })
                .collect();
            let mut chunks = Vec::new();
            for w in work {
                let done = w
                    .join()
                    .map_err(|_| Error::InvalidCommand("writing a column failed".into()))??;
                chunks.extend(done);
            }
            Ok::<_, Error>(chunks)
        })?;
        let mut row_group = self.writer.next_row_group()?;
        for chunk in chunks {
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
