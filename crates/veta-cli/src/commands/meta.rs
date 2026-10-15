//! Metadata and writer settings: `meta` and `writer-settings`.

use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use veta_core::model::KeyValue;
use veta_core::{Command, Compression, Document, Encoding, FormatVersion, StatisticsLevel};

use super::*;
use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum MetaCommand {
    /// List all key/value metadata.
    List {
        file: PathBuf,
        #[command(flatten)]
        scope: Scope,
    },
    /// Print one value.
    Get {
        file: PathBuf,
        key: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// Set a value, adding the key if needed.
    Set {
        file: PathBuf,
        key: String,
        value: String,
        #[command(flatten)]
        scope: Scope,
        #[command(flatten)]
        output: Output,
    },
    /// Remove a key.
    Remove {
        file: PathBuf,
        key: String,
        #[command(flatten)]
        scope: Scope,
        #[command(flatten)]
        output: Output,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct Scope {
    /// A column's metadata (stored in the schema) instead of the file's.
    #[arg(short, long, value_name = "NAME")]
    pub column: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompressionArg {
    Uncompressed,
    Snappy,
    Gzip,
    Brotli,
    Lz4,
    Lz4Raw,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EncodingArg {
    Plain,
    DeltaBinaryPacked,
    DeltaLengthByteArray,
    DeltaByteArray,
    ByteStreamSplit,
    Rle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StatisticsArg {
    None,
    Chunk,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Toggle {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    #[value(name = "1")]
    V1,
    #[value(name = "2")]
    V2,
}

/// Options of `veta-cli writer-settings`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct WriterArgs {
    pub file: PathBuf,
    /// Columns to change (repeatable). Without it, column options apply to
    /// every column and to new columns.
    #[arg(long = "column", value_name = "NAME")]
    pub columns: Vec<String>,
    #[arg(long)]
    pub compression: Option<CompressionArg>,
    /// Compression level (gzip 0-10, brotli 0-11, zstd 1-22).
    #[arg(long)]
    pub level: Option<i32>,
    #[arg(long)]
    pub encoding: Option<EncodingArg>,
    #[arg(long)]
    pub dictionary: Option<Toggle>,
    #[arg(long)]
    pub statistics: Option<StatisticsArg>,
    #[arg(long)]
    pub bloom_filter: Option<Toggle>,
    /// Maximum rows per row group.
    #[arg(long)]
    pub row_group_rows: Option<usize>,
    #[arg(long)]
    pub format_version: Option<FormatArg>,
    #[command(flatten)]
    pub output: Output,
}

impl WriterArgs {
    fn changes_anything(&self) -> bool {
        self.compression.is_some()
            || self.level.is_some()
            || self.encoding.is_some()
            || self.dictionary.is_some()
            || self.statistics.is_some()
            || self.bloom_filter.is_some()
            || self.row_group_rows.is_some()
            || self.format_version.is_some()
    }
}

pub fn meta(command: MetaCommand, out: &mut impl Write) -> Result<(), Error> {
    match command {
        MetaCommand::List { file, scope } => {
            let doc = crate::open(&file)?;
            for kv in metadata_of(&doc, &scope)? {
                writeln!(out, "{} = {}", kv.key, kv.value.as_deref().unwrap_or(""))?;
            }
            Ok(())
        }
        MetaCommand::Get { file, key, scope } => {
            let doc = crate::open(&file)?;
            match metadata_of(&doc, &scope)?
                .into_iter()
                .find(|kv| kv.key == key)
            {
                Some(KeyValue { value, .. }) => {
                    writeln!(out, "{}", value.as_deref().unwrap_or(""))?;
                    Ok(())
                }
                None => Err(Error::Usage(format!("no metadata key {key:?}"))),
            }
        }
        MetaCommand::Set {
            file,
            key,
            value,
            scope,
            output,
        } => edit(&file, &output, out, |doc| match &scope.column {
            None => Ok(vec![Command::SetMetadata {
                key,
                value: Some(value),
            }]),
            Some(column) => {
                let mut entries = metadata_of(doc, &scope)?;
                match entries.iter_mut().find(|kv| kv.key == key) {
                    Some(kv) => kv.value = Some(value),
                    None => entries.push(KeyValue {
                        key,
                        value: Some(value),
                    }),
                }
                Ok(vec![Command::ReplaceColumnMetadata {
                    column: column.clone(),
                    entries,
                }])
            }
        }),
        MetaCommand::Remove {
            file,
            key,
            scope,
            output,
        } => edit(&file, &output, out, |doc| match &scope.column {
            None => Ok(vec![Command::RemoveMetadata { key }]),
            Some(column) => {
                let mut entries = metadata_of(doc, &scope)?;
                let before = entries.len();
                entries.retain(|kv| kv.key != key);
                if entries.len() == before {
                    return Err(Error::Usage(format!("no metadata key {key:?}")));
                }
                Ok(vec![Command::ReplaceColumnMetadata {
                    column: column.clone(),
                    entries,
                }])
            }
        }),
    }
}

/// The file's metadata, or a column's (sorted by key).
fn metadata_of(doc: &Document, scope: &Scope) -> Result<Vec<KeyValue>, Error> {
    let Some(column) = &scope.column else {
        return Ok(doc.metadata().key_value.clone());
    };
    if let Some(entries) = doc.metadata().column(column) {
        return Ok(entries.to_vec());
    }
    let schema = doc.schema();
    let field = schema
        .field_with_name(column)
        .map_err(|_| Error::Usage(format!("no column named {column:?}")))?;
    let mut entries: Vec<KeyValue> = field
        .metadata()
        .iter()
        .map(|(k, v)| KeyValue {
            key: k.clone(),
            value: Some(v.clone()),
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(entries)
}

pub fn writer_settings(args: WriterArgs, out: &mut impl Write) -> Result<(), Error> {
    if !args.changes_anything() {
        let doc = crate::open(&args.file)?;
        return super::inspect::print_writer_settings(&doc, out);
    }
    if args.level.is_some() && args.compression.is_none() {
        return Err(Error::Usage("--level needs --compression".into()));
    }
    let compression = args
        .compression
        .map(|c| compression(c, args.level))
        .transpose()?;
    let output = args.output.clone();
    edit(&args.file.clone(), &output, out, |doc| {
        let schema = doc.schema();
        let mut settings = doc.writer_settings().clone();
        if let Some(rows) = args.row_group_rows {
            settings.max_row_group_rows = rows;
        }
        if let Some(version) = args.format_version {
            settings.format_version = match version {
                FormatArg::V1 => FormatVersion::V1,
                FormatArg::V2 => FormatVersion::V2,
            };
        }
        let targets: Vec<String> = if args.columns.is_empty() {
            schema.fields().iter().map(|f| f.name().clone()).collect()
        } else {
            args.columns.clone()
        };
        let encoding = args.encoding.map(encoding);
        for name in &targets {
            let field = schema
                .field_with_name(name)
                .map_err(|_| Error::Usage(format!("no column named {name:?}")))?;
            let mut column = *settings.column(name);
            if let Some(c) = compression {
                column.compression = c;
            }
            if let Some(e) = encoding {
                if Encoding::choices_for(field.data_type()).contains(&e) {
                    column.encoding = e;
                } else if !args.columns.is_empty() {
                    return Err(Error::Usage(format!(
                        "{e} encoding does not apply to column {name}"
                    )));
                }
            }
            if let Some(d) = args.dictionary {
                column.dictionary = d == Toggle::On;
            }
            if let Some(s) = args.statistics {
                column.statistics = statistics(s);
            }
            if let Some(b) = args.bloom_filter {
                column.bloom_filter = b == Toggle::On;
            }
            match settings.columns.iter_mut().find(|(p, _)| p == name) {
                Some((_, existing)) => *existing = column,
                None => settings.columns.push((name.clone(), column)),
            }
        }
        if args.columns.is_empty() {
            let d = &mut settings.default_column;
            if let Some(c) = compression {
                d.compression = c;
            }
            if let Some(v) = args.dictionary {
                d.dictionary = v == Toggle::On;
            }
            if let Some(s) = args.statistics {
                d.statistics = statistics(s);
            }
            if let Some(b) = args.bloom_filter {
                d.bloom_filter = b == Toggle::On;
            }
        }
        Ok(vec![Command::SetWriterSettings(settings)])
    })
}

fn compression(c: CompressionArg, level: Option<i32>) -> Result<Compression, Error> {
    let unsigned = |level: Option<i32>| {
        level
            .map(|l| u32::try_from(l).map_err(|_| Error::Usage("level must be 0 or more".into())))
            .transpose()
    };
    let no_level = |c: Compression| match level {
        Some(_) => Err(Error::Usage(format!("{c} has no compression levels"))),
        None => Ok(c),
    };
    match c {
        CompressionArg::Uncompressed => no_level(Compression::Uncompressed),
        CompressionArg::Snappy => no_level(Compression::Snappy),
        CompressionArg::Lz4 => no_level(Compression::Lz4),
        CompressionArg::Lz4Raw => no_level(Compression::Lz4Raw),
        CompressionArg::Gzip => Ok(Compression::Gzip(unsigned(level)?)),
        CompressionArg::Brotli => Ok(Compression::Brotli(unsigned(level)?)),
        CompressionArg::Zstd => Ok(Compression::Zstd(level)),
    }
}

fn encoding(e: EncodingArg) -> Encoding {
    match e {
        EncodingArg::Plain => Encoding::Plain,
        EncodingArg::DeltaBinaryPacked => Encoding::DeltaBinaryPacked,
        EncodingArg::DeltaLengthByteArray => Encoding::DeltaLengthByteArray,
        EncodingArg::DeltaByteArray => Encoding::DeltaByteArray,
        EncodingArg::ByteStreamSplit => Encoding::ByteStreamSplit,
        EncodingArg::Rle => Encoding::Rle,
    }
}

fn statistics(s: StatisticsArg) -> StatisticsLevel {
    match s {
        StatisticsArg::None => StatisticsLevel::None,
        StatisticsArg::Chunk => StatisticsLevel::Chunk,
        StatisticsArg::Page => StatisticsLevel::Page,
    }
}
