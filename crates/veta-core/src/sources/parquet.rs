//! Reading a Parquet file.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::datatypes::Schema;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder,
};
use parquet::file::metadata::PageIndexPolicy;

use crate::error::{Result, invalid};
use crate::files::parquet::{check_row_counts, file_info, file_metadata, writer_settings};
use crate::plan::store::Table;
use crate::plan::{Effect, FileSettings, Planned, Shape, Step};
use crate::source::{DataSource, PagedSource};

/// Reads a Parquet file, or only `columns` of it.
#[derive(Debug)]
pub(crate) struct ReadParquet {
    pub(crate) path: PathBuf,
    pub(crate) columns: Option<Vec<String>>,
}

impl Step for ReadParquet {
    /// Reads the footer, and the whole file unless `lazy` or it decodes to
    /// more than the memory budget (it is then read as needed).
    fn plan(&self, _input: &Shape, lazy: bool) -> Result<Planned> {
        let file = File::open(&self.path)?;
        let file_size = file.metadata()?.len();
        let options = ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Optional);
        let metadata = ArrowReaderMetadata::load(&file, options)?;
        let footer = metadata.metadata().clone();
        check_row_counts(&footer)?;
        let all = metadata.schema().clone();
        let positions = match &self.columns {
            Some(names) => names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    if names[..i].contains(name) {
                        return Err(invalid!(ColumnTwice, column = name));
                    }
                    all.index_of(name)
                        .map_err(|_| invalid!(ColumnNotFound, column = name))
                })
                .collect::<Result<Vec<_>>>()?,
            None => (0..all.fields().len()).collect(),
        };
        let schema = Arc::new(all.project(&positions)?);
        let budget = crate::memory::budget();
        // The footer's sizes can't be trusted: the file is read into memory
        // only while what it decodes to stays within the budget.
        let fits = usize::try_from(
            file_info(&footer, file_size)
                .uncompressed_size()
                .max(file_size),
        )
        .is_ok_and(|s| s <= budget);
        let in_memory = match fits && !lazy {
            true => read_within(&file, &metadata, &schema, &positions, budget)?,
            false => None,
        };
        let data: Arc<dyn DataSource> = match in_memory {
            Some(table) => table,
            None => Arc::new(
                PagedSource::new(file, metadata, crate::memory::page_cache(budget))
                    .with_path(&self.path),
            ),
        };
        Ok(Planned {
            schema,
            effect: Effect::Data(data),
            file: Some(FileSettings {
                metadata: file_metadata(&footer),
                writer: writer_settings(&footer),
            }),
        })
    }
}

/// The top-level columns at `positions` of the file, kept as a table, or
/// `None` if they decode to more than `budget` bytes.
fn read_within(
    file: &File,
    metadata: &ArrowReaderMetadata,
    schema: &Arc<Schema>,
    positions: &[usize],
    budget: usize,
) -> Result<Option<Arc<Table>>> {
    let mask = ProjectionMask::roots(
        metadata.metadata().file_metadata().schema_descr(),
        positions.iter().copied(),
    );
    let reader =
        ParquetRecordBatchReaderBuilder::new_with_metadata(file.try_clone()?, metadata.clone())
            .with_projection(mask)
            .build()?;
    // Columns come in the file's order: put them in the order asked for.
    let mut in_file = positions.to_vec();
    in_file.sort_unstable();
    let order: Vec<usize> = positions
        .iter()
        .map(|p| in_file.partition_point(|q| q < p))
        .collect();
    let mut batches = Vec::new();
    let mut bytes = 0usize;
    for batch in reader {
        let batch = batch?;
        bytes = bytes.saturating_add(batch.get_array_memory_size());
        if bytes > budget {
            return Ok(None);
        }
        batches.push(batch.project(&order)?.with_schema(schema.clone())?);
    }
    Ok(Some(Table::new(schema.clone(), batches)?))
}
