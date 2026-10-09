/// File-level metadata stored in the Parquet footer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMetadata {
    /// Key/value pairs, in file order. Parquet allows a key without a value.
    pub key_value: Vec<KeyValue>,
    /// The `created_by` string of the writer that produced the file.
    pub created_by: Option<String>,
}

impl FileMetadata {
    pub fn get(&self, key: &str) -> Option<&KeyValue> {
        self.key_value.iter().find(|kv| kv.key == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: Option<String>,
}

/// Physical layout of the file a document was opened from. Read-only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileInfo {
    /// Size of the file on disk, in bytes.
    pub file_size: u64,
    pub row_groups: Vec<RowGroupInfo>,
}

impl FileInfo {
    /// Sum of the uncompressed sizes of all row groups.
    pub fn uncompressed_size(&self) -> u64 {
        self.row_groups.iter().map(|rg| rg.uncompressed_size).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowGroupInfo {
    pub num_rows: usize,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

/// Settings used when writing the document back to Parquet.
///
/// For opened files these are recovered from the file's metadata so that
/// saving keeps the original layout. New documents get [`Default`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterSettings {
    pub format_version: FormatVersion,
    /// Maximum rows per row group.
    pub max_row_group_rows: usize,
    /// Settings applied to columns without an entry in `columns`.
    pub default_column: ColumnSettings,
    /// Per-column settings, keyed by dotted column path (`a.b` for nested).
    pub columns: Vec<(String, ColumnSettings)>,
}

impl WriterSettings {
    /// Settings for a column, falling back to [`Self::default_column`].
    pub fn column(&self, path: &str) -> &ColumnSettings {
        self.columns
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s)
            .unwrap_or(&self.default_column)
    }
}

impl Default for WriterSettings {
    fn default() -> Self {
        Self {
            format_version: FormatVersion::V2,
            max_row_group_rows: 1024 * 1024,
            default_column: ColumnSettings::default(),
            columns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatVersion {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSettings {
    pub compression: Compression,
    /// Data encoding used when not dictionary-encoded, or when the dictionary
    /// falls back.
    pub encoding: Encoding,
    pub dictionary: bool,
    pub statistics: StatisticsLevel,
    pub bloom_filter: bool,
}

impl Default for ColumnSettings {
    fn default() -> Self {
        Self {
            compression: Compression::Snappy,
            encoding: Encoding::Plain,
            dictionary: true,
            statistics: StatisticsLevel::Page,
            bloom_filter: false,
        }
    }
}

/// Compression codec. Levels are not stored in Parquet files, so codecs with
/// levels carry `None` when read from a file (meaning the codec's default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Uncompressed,
    Snappy,
    Gzip(Option<u32>),
    Lzo,
    Brotli(Option<u32>),
    Lz4,
    Zstd(Option<i32>),
    Lz4Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Plain,
    DeltaBinaryPacked,
    DeltaLengthByteArray,
    DeltaByteArray,
    ByteStreamSplit,
    Rle,
}

/// Which statistics are written for a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatisticsLevel {
    None,
    /// Per column chunk (row group).
    Chunk,
    /// Per column chunk and per page (column index).
    Page,
}

impl std::fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FormatVersion::V1 => "1.0",
            FormatVersion::V2 => "2.x",
        })
    }
}

impl std::fmt::Display for Compression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (name, level) = match *self {
            Compression::Uncompressed => ("uncompressed", None),
            Compression::Snappy => ("snappy", None),
            Compression::Gzip(level) => ("gzip", level.map(i64::from)),
            Compression::Lzo => ("lzo", None),
            Compression::Brotli(level) => ("brotli", level.map(i64::from)),
            Compression::Lz4 => ("lz4", None),
            Compression::Zstd(level) => ("zstd", level.map(i64::from)),
            Compression::Lz4Raw => ("lz4_raw", None),
        };
        match level {
            Some(level) => write!(f, "{name}({level})"),
            None => f.write_str(name),
        }
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Encoding::Plain => "plain",
            Encoding::DeltaBinaryPacked => "delta_binary_packed",
            Encoding::DeltaLengthByteArray => "delta_length_byte_array",
            Encoding::DeltaByteArray => "delta_byte_array",
            Encoding::ByteStreamSplit => "byte_stream_split",
            Encoding::Rle => "rle",
        })
    }
}

impl std::fmt::Display for StatisticsLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StatisticsLevel::None => "none",
            StatisticsLevel::Chunk => "chunk",
            StatisticsLevel::Page => "page",
        })
    }
}
