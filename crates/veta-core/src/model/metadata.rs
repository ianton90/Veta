/// File-level metadata stored in the Parquet footer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMetadata {
    /// Key/value pairs, in file order. Parquet allows a key without a value.
    pub key_value: Vec<KeyValue>,
    /// The `created_by` string of the writer that produced the file.
    pub created_by: Option<String>,
    /// Column (field) metadata edited in Veta, by top-level column name. It
    /// replaces the column's own metadata on save; columns not listed keep
    /// theirs.
    pub columns: Vec<(String, Vec<KeyValue>)>,
}

impl FileMetadata {
    pub fn get(&self, key: &str) -> Option<&KeyValue> {
        self.key_value.iter().find(|kv| kv.key == key)
    }

    /// Metadata edited for `column`, if any.
    pub fn column(&self, column: &str) -> Option<&[KeyValue]> {
        self.columns
            .iter()
            .find(|(name, _)| name == column)
            .map(|(_, entries)| entries.as_slice())
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

impl Compression {
    /// Codecs offered when choosing a compression (LZO cannot be written).
    pub const CHOICES: [Compression; 7] = [
        Compression::Uncompressed,
        Compression::Snappy,
        Compression::Gzip(None),
        Compression::Brotli(None),
        Compression::Lz4,
        Compression::Lz4Raw,
        Compression::Zstd(None),
    ];

    /// Levels the codec accepts, if it takes one.
    pub fn level_range(self) -> Option<std::ops::RangeInclusive<i32>> {
        match self {
            Compression::Gzip(_) => Some(0..=10),
            Compression::Brotli(_) => Some(0..=11),
            Compression::Zstd(_) => Some(1..=22),
            _ => None,
        }
    }

    /// The level, `None` for the codec's default (or no level).
    pub fn level(self) -> Option<i32> {
        match self {
            Compression::Gzip(l) | Compression::Brotli(l) => l.and_then(|l| i32::try_from(l).ok()),
            Compression::Zstd(l) => l,
            _ => None,
        }
    }

    /// The same codec with `level` (ignored by codecs without levels).
    pub fn with_level(self, level: Option<i32>) -> Compression {
        let unsigned = level.and_then(|l| u32::try_from(l).ok());
        match self {
            Compression::Gzip(_) => Compression::Gzip(unsigned),
            Compression::Brotli(_) => Compression::Brotli(unsigned),
            Compression::Zstd(_) => Compression::Zstd(level),
            other => other,
        }
    }

    /// The same codec without a level.
    pub fn without_level(self) -> Compression {
        match self {
            Compression::Gzip(_) => Compression::Gzip(None),
            Compression::Brotli(_) => Compression::Brotli(None),
            Compression::Zstd(_) => Compression::Zstd(None),
            other => other,
        }
    }
}

impl std::str::FromStr for Compression {
    type Err = String;

    /// Parses the [`Display`](std::fmt::Display) form: `zstd`, `zstd(3)`.
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim().to_ascii_lowercase();
        let (name, level) = match s.strip_suffix(')').and_then(|s| s.split_once('(')) {
            Some((name, level)) => {
                let level: i32 = level
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad compression level in {s:?}"))?;
                (name.trim().to_owned(), Some(level))
            }
            None => (s.clone(), None),
        };
        let codec = Compression::CHOICES
            .into_iter()
            .find(|c| c.to_string() == name)
            .ok_or_else(|| format!("unknown compression {name:?}"))?;
        if let (Some(level), Some(range)) = (level, codec.level_range())
            && !range.contains(&level)
        {
            return Err(format!(
                "{name} level must be {} to {}",
                range.start(),
                range.end()
            ));
        }
        Ok(codec.with_level(level))
    }
}

impl std::str::FromStr for StatisticsLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|l| l.to_string() == s.trim())
            .ok_or_else(|| format!("unknown statistics level {s:?}"))
    }
}

impl std::str::FromStr for FormatVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim() {
            "1" | "1.0" => Ok(Self::V1),
            "2" | "2.x" | "2.0" => Ok(Self::V2),
            _ => Err(format!("unknown format version {s:?}")),
        }
    }
}

impl StatisticsLevel {
    pub const ALL: [StatisticsLevel; 3] = [Self::None, Self::Chunk, Self::Page];
}

impl FormatVersion {
    pub const ALL: [FormatVersion; 2] = [Self::V1, Self::V2];
}

impl Encoding {
    /// Encodings that apply to columns of `data_type`.
    pub fn choices_for(data_type: &arrow::datatypes::DataType) -> Vec<Encoding> {
        use arrow::datatypes::DataType as T;
        let mut choices = vec![Encoding::Plain];
        match data_type {
            T::Int8
            | T::Int16
            | T::Int32
            | T::Int64
            | T::UInt8
            | T::UInt16
            | T::UInt32
            | T::UInt64
            | T::Date32
            | T::Date64
            | T::Time32(_)
            | T::Time64(_)
            | T::Timestamp(..)
            | T::Duration(_)
            | T::Decimal32(..)
            | T::Decimal64(..) => {
                choices.extend([Encoding::DeltaBinaryPacked, Encoding::ByteStreamSplit]);
            }
            T::Float16 | T::Float32 | T::Float64 => choices.push(Encoding::ByteStreamSplit),
            T::Utf8 | T::LargeUtf8 | T::Utf8View | T::Binary | T::LargeBinary | T::BinaryView => {
                choices.extend([Encoding::DeltaLengthByteArray, Encoding::DeltaByteArray]);
            }
            T::Boolean => choices.push(Encoding::Rle),
            _ => {}
        }
        choices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_round_trips_through_text() {
        for c in [
            Compression::Snappy,
            Compression::Zstd(Some(3)),
            Compression::Gzip(None),
            Compression::Brotli(Some(11)),
        ] {
            assert_eq!(c.to_string().parse::<Compression>(), Ok(c));
        }
        assert!("zstd(30)".parse::<Compression>().is_err());
        assert!("lzo".parse::<Compression>().is_err(), "can't be written");
        assert_eq!(Compression::Zstd(Some(3)).level(), Some(3));
        assert_eq!(Compression::Snappy.with_level(Some(3)), Compression::Snappy);
        assert_eq!("2.x".parse(), Ok(FormatVersion::V2));
        assert_eq!("chunk".parse(), Ok(StatisticsLevel::Chunk));
    }
}
