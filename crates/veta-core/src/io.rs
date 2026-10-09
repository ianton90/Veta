//! Reading Parquet files into documents.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder,
};
use parquet::basic::{Compression as PqCompression, Encoding as PqEncoding};
use parquet::file::metadata::{ColumnChunkMetaData, PageIndexPolicy, ParquetMetaData};

use crate::error::Result;
use crate::model::{
    ColumnSettings, Compression, Encoding, FileInfo, FileMetadata, FormatVersion, KeyValue,
    RowGroupInfo, StatisticsLevel, WriterSettings,
};
use crate::source::{DataSource, MemorySource, PagedSource};

/// Default memory budget for loading a file: 1 GiB.
pub const DEFAULT_MEMORY_BUDGET: usize = 1024 * 1024 * 1024;

/// How to open a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// Files whose uncompressed size fits in this many bytes are loaded fully
    /// into memory; larger files are paged, with the page cache bounded by
    /// the same budget.
    pub memory_budget: usize,
    /// Always page, regardless of size. Useful when only a few rows are
    /// needed (e.g. `veta head`).
    pub force_paged: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            memory_budget: DEFAULT_MEMORY_BUDGET,
            force_paged: false,
        }
    }
}

impl OpenOptions {
    /// Paged with a small cache: for reading only a few rows.
    pub fn paged() -> Self {
        Self {
            memory_budget: 64 * 1024 * 1024,
            force_paged: true,
        }
    }
}

/// Everything read from a Parquet file when opening it.
#[derive(Debug)]
pub struct OpenedFile {
    pub source: Arc<dyn DataSource>,
    pub metadata: FileMetadata,
    pub writer: WriterSettings,
    pub info: FileInfo,
}

/// Opens a Parquet file: reads its footer and builds an in-memory or paged
/// source depending on `options`.
pub fn open_parquet(path: &Path, options: OpenOptions) -> Result<OpenedFile> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let reader_options =
        ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Optional);
    let arrow_metadata = ArrowReaderMetadata::load(&file, reader_options)?;
    let parquet_metadata = arrow_metadata.metadata().clone();

    let info = file_info(&parquet_metadata, file_size);
    let fits = usize::try_from(info.uncompressed_size()).is_ok_and(|s| s <= options.memory_budget);

    let source: Arc<dyn DataSource> = if fits && !options.force_paged {
        let schema = arrow_metadata.schema().clone();
        let reader =
            ParquetRecordBatchReaderBuilder::new_with_metadata(file, arrow_metadata).build()?;
        let batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
        Arc::new(MemorySource::new(schema, batches)?)
    } else {
        Arc::new(PagedSource::new(
            file,
            arrow_metadata,
            options.memory_budget,
        ))
    };

    Ok(OpenedFile {
        source,
        metadata: file_metadata(&parquet_metadata),
        writer: writer_settings(&parquet_metadata),
        info,
    })
}

fn file_info(metadata: &ParquetMetaData, file_size: u64) -> FileInfo {
    FileInfo {
        file_size,
        row_groups: metadata
            .row_groups()
            .iter()
            .map(|rg| RowGroupInfo {
                num_rows: usize::try_from(rg.num_rows()).unwrap_or(0),
                compressed_size: u64::try_from(rg.compressed_size()).unwrap_or(0),
                uncompressed_size: u64::try_from(rg.total_byte_size()).unwrap_or(0),
            })
            .collect(),
    }
}

fn file_metadata(metadata: &ParquetMetaData) -> FileMetadata {
    let fm = metadata.file_metadata();
    FileMetadata {
        key_value: fm
            .key_value_metadata()
            .map(|kvs| {
                kvs.iter()
                    .map(|kv| KeyValue {
                        key: kv.key.clone(),
                        value: kv.value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        created_by: fm.created_by().map(str::to_owned),
    }
}

/// Recovers writer settings from the file's metadata.
///
/// Column settings come from the first row group (writers use the same
/// settings for every row group). The row-group size is the largest row
/// group's row count. Things Parquet does not record (compression levels,
/// page size limits) are left at their defaults.
fn writer_settings(metadata: &ParquetMetaData) -> WriterSettings {
    let defaults = WriterSettings::default();
    let format_version = if metadata.file_metadata().version() >= 2 {
        FormatVersion::V2
    } else {
        FormatVersion::V1
    };
    let max_row_group_rows = metadata
        .row_groups()
        .iter()
        .filter_map(|rg| usize::try_from(rg.num_rows()).ok())
        .max()
        .filter(|&n| n > 0)
        .unwrap_or(defaults.max_row_group_rows);
    let columns = metadata
        .row_groups()
        .first()
        .map(|rg| {
            rg.columns()
                .iter()
                .map(|c| (c.column_path().string(), column_settings(c)))
                .collect()
        })
        .unwrap_or_default();

    WriterSettings {
        format_version,
        max_row_group_rows,
        default_column: defaults.default_column,
        columns,
    }
}

fn column_settings(column: &ColumnChunkMetaData) -> ColumnSettings {
    let encodings: Vec<PqEncoding> = column.encodings().collect();
    #[allow(deprecated)] // PLAIN_DICTIONARY is deprecated but still found in old files.
    let dictionary = column.dictionary_page_offset().is_some()
        || encodings
            .iter()
            .any(|e| matches!(e, PqEncoding::RLE_DICTIONARY | PqEncoding::PLAIN_DICTIONARY));

    // RLE is listed for definition/repetition levels and PLAIN for dictionary
    // pages, so a specialised encoding, if present, is the data encoding.
    let encoding = encodings
        .iter()
        .find_map(|e| match e {
            PqEncoding::DELTA_BINARY_PACKED => Some(Encoding::DeltaBinaryPacked),
            PqEncoding::DELTA_LENGTH_BYTE_ARRAY => Some(Encoding::DeltaLengthByteArray),
            PqEncoding::DELTA_BYTE_ARRAY => Some(Encoding::DeltaByteArray),
            PqEncoding::BYTE_STREAM_SPLIT => Some(Encoding::ByteStreamSplit),
            _ => None,
        })
        .unwrap_or(Encoding::Plain);

    let statistics = if column.column_index_offset().is_some() {
        StatisticsLevel::Page
    } else if column.statistics().is_some() {
        StatisticsLevel::Chunk
    } else {
        StatisticsLevel::None
    };

    ColumnSettings {
        compression: compression(column.compression()),
        encoding,
        dictionary,
        statistics,
        bloom_filter: column.bloom_filter_offset().is_some(),
    }
}

fn compression(c: PqCompression) -> Compression {
    match c {
        PqCompression::UNCOMPRESSED => Compression::Uncompressed,
        PqCompression::SNAPPY => Compression::Snappy,
        PqCompression::GZIP(_) => Compression::Gzip(None),
        PqCompression::LZO => Compression::Lzo,
        PqCompression::BROTLI(_) => Compression::Brotli(None),
        PqCompression::LZ4 => Compression::Lz4,
        PqCompression::ZSTD(_) => Compression::Zstd(None),
        PqCompression::LZ4_RAW => Compression::Lz4Raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceMode;
    use veta_testkit::{TempDir, fixtures};

    #[test]
    fn small_file_opens_in_memory() {
        let dir = TempDir::new();
        let path = dir.join("all.parquet");
        fixtures::all_types(&path);

        let opened = open_parquet(&path, OpenOptions::default()).unwrap();
        assert_eq!(opened.source.mode(), SourceMode::InMemory);
        assert_eq!(opened.source.num_rows(), fixtures::ALL_TYPES_ROWS);
        assert_eq!(opened.source.schema().fields().len(), 19);
        assert_eq!(opened.info.row_groups.len(), 1);
        assert!(opened.info.file_size > 0);
    }

    #[test]
    fn file_over_budget_opens_paged() {
        let dir = TempDir::new();
        let path = dir.join("large.parquet");
        fixtures::large(&path, 100_000, 25_000);

        let options = OpenOptions {
            memory_budget: 1024,
            force_paged: false,
        };
        let opened = open_parquet(&path, options).unwrap();
        assert_eq!(opened.source.mode(), SourceMode::Paged);
        assert_eq!(opened.source.num_rows(), 100_000);
        assert_eq!(opened.source.read(99_999..100_000).unwrap().num_rows(), 1);
    }

    #[test]
    fn reads_metadata_and_writer_settings() {
        let dir = TempDir::new();
        let path = dir.join("mixed.parquet");
        fixtures::mixed_settings(&path);

        let opened = open_parquet(&path, OpenOptions::default()).unwrap();
        for (key, value) in fixtures::MIXED_SETTINGS_METADATA {
            assert_eq!(
                opened.metadata.get(key).unwrap().value.as_deref(),
                Some(*value)
            );
        }
        assert!(opened.metadata.created_by.is_some());

        let w = &opened.writer;
        assert_eq!(w.max_row_group_rows, fixtures::MIXED_SETTINGS_ROW_GROUP);
        assert_eq!(w.columns.len(), 4);

        let id = w.column("id");
        assert_eq!(id.compression, Compression::Zstd(None));
        assert_eq!(id.encoding, Encoding::DeltaBinaryPacked);
        assert!(!id.dictionary);
        assert_eq!(id.statistics, StatisticsLevel::Page);

        let name = w.column("name");
        assert_eq!(name.compression, Compression::Snappy);
        assert!(name.dictionary);
        assert_eq!(name.statistics, StatisticsLevel::Chunk);

        let score = w.column("score");
        assert_eq!(score.compression, Compression::Gzip(None));
        assert_eq!(score.encoding, Encoding::ByteStreamSplit);
        assert_eq!(score.statistics, StatisticsLevel::None);

        let flag = w.column("flag");
        assert_eq!(flag.compression, Compression::Uncompressed);
        assert_eq!(flag.encoding, Encoding::Plain);
        assert!(!flag.dictionary);
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let err = open_parquet(Path::new("/nonexistent/x.parquet"), OpenOptions::default());
        assert!(matches!(err, Err(crate::Error::Io(_))));
    }

    #[test]
    fn non_parquet_file_is_a_parquet_error() {
        let dir = TempDir::new();
        let path = dir.join("bad.parquet");
        std::fs::write(&path, b"definitely not parquet").unwrap();
        let err = open_parquet(&path, OpenOptions::default());
        assert!(matches!(err, Err(crate::Error::Parquet(_))));
    }
}
