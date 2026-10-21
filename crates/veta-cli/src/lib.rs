//! Veta's command-line interface (the `veta-cli` binary). Parses arguments
//! with clap and runs headless commands through `veta-core`.

mod commands;
mod names;
mod table;

use commands::{apply, convert, edit, inspect, meta};

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use veta_core::{Document, OpenOptions};

/// Read and edit Parquet files from the command line. The desktop app is
/// `veta`.
#[derive(Debug, Parser)]
#[command(name = "veta-cli", version = veta_core::version::LONG)]
struct Cli {
    /// Memory Veta may use for data, e.g. 4G, 512M or 25% (default: half
    /// of RAM).
    #[arg(long, global = true, value_name = "SIZE", value_parser = parse_memory)]
    memory: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

fn parse_memory(text: &str) -> Result<usize, String> {
    veta_core::memory::parse_budget(text).map_err(|e| e.to_string())
}

/// A headless command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Show file layout, writer settings and metadata.
    Info { file: PathBuf },
    /// Show the schema.
    Schema { file: PathBuf },
    /// Print the first rows.
    Head {
        file: PathBuf,
        /// Number of rows to print.
        #[arg(short = 'n', long, default_value_t = 10)]
        rows: usize,
    },
    /// Set one cell. Rows are numbered from 1.
    SetCell {
        file: PathBuf,
        #[arg(long)]
        row: usize,
        #[arg(long)]
        column: String,
        /// New value, parsed as the column's type.
        #[arg(long, required_unless_present = "null", conflicts_with = "null")]
        value: Option<String>,
        /// Set the cell to null instead.
        #[arg(long)]
        null: bool,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Insert empty rows at the top, at the bottom, or before row AT (the
    /// row count + 1 also appends).
    #[command(group(clap::ArgGroup::new("place").required(true).args(["top", "bottom", "at"])))]
    InsertRows {
        file: PathBuf,
        #[arg(long)]
        top: bool,
        #[arg(long)]
        bottom: bool,
        #[arg(long)]
        at: Option<usize>,
        /// How many rows.
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Remove rows: the first N, the last N, or row ranges, e.g. --rows 5
    /// --rows 10-20 or --rows 5,10-20.
    #[command(name = "remove-rows", visible_alias = "delete-rows")]
    DeleteRows {
        #[command(flatten)]
        rows: edit::RowsArgs,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Keep only some rows: the first N, the last N, or row ranges (as for
    /// remove-rows).
    KeepRows {
        #[command(flatten)]
        rows: edit::RowsArgs,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Add an empty column.
    AddColumn {
        file: PathBuf,
        name: String,
        /// Type, e.g. string, int64, float64, bool, date, timestamp[ms].
        #[arg(long = "type", value_name = "TYPE")]
        data_type: String,
        /// Position, from 1 (default: last).
        #[arg(long)]
        at: Option<usize>,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Remove columns.
    RemoveColumn {
        file: PathBuf,
        #[arg(required = true)]
        names: Vec<String>,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Rename a column.
    RenameColumn {
        file: PathBuf,
        from: String,
        to: String,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Move a column to a position, from 1.
    MoveColumn {
        file: PathBuf,
        name: String,
        #[arg(long)]
        to: usize,
        #[command(flatten)]
        output: commands::Output,
    },
    /// Read or change key/value metadata.
    Meta {
        #[command(subcommand)]
        command: meta::MetaCommand,
    },
    /// Show writer settings, or change them with the options below.
    WriterSettings(meta::WriterArgs),
    /// Column statistics: count, nulls, distinct, min, max, mean, std dev.
    Stats {
        file: PathBuf,
        /// Only these columns (repeatable; default: all).
        #[arg(short, long = "column", value_name = "NAME")]
        columns: Vec<String>,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Convert between Parquet, CSV and JSON (formats from the file
    /// extensions unless --format is given).
    Convert(convert::ConvertArgs),
    /// Apply a recipe (steps saved from the desktop app) to a file.
    Apply {
        /// Recipe file (.json).
        recipe: PathBuf,
        file: PathBuf,
        #[command(flatten)]
        output: commands::Output,
    },
}

/// Parses the process arguments. Prints help/version or an argument error
/// and exits the process when clap requires it.
pub fn parse() -> Command {
    parse_from(std::env::args_os())
}

/// Like [`parse`], for the given arguments (the first is the program name).
pub fn parse_from<I, T>(args: I) -> Command
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);
    veta_core::memory::set_budget(cli.memory);
    cli.command
}

pub fn run(command: Command) -> ExitCode {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let start = std::time::Instant::now();
    let result = execute(command, &mut out);
    // With VETA_PROFILE set: where the time went.
    if let Some(report) = veta_core::profile::report() {
        eprintln!(
            "{report}  {:<40} {:>9.2} s",
            "total",
            start.elapsed().as_secs_f64()
        );
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // Broken pipe (e.g. `veta-cli head f | head -1`) is not an error.
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            let _ = out.flush();
            eprintln!("veta-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Core(veta_core::Error),
    Io(std::io::Error),
    Usage(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Core(e) => e.fmt(f),
            Error::Io(e) => e.fmt(f),
            Error::Usage(msg) => f.write_str(msg),
        }
    }
}

impl From<veta_core::Error> for Error {
    fn from(e: veta_core::Error) -> Self {
        Error::Core(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

fn execute(command: Command, out: &mut impl Write) -> Result<(), Error> {
    match command {
        Command::Info { file } => inspect::info(&open(&file)?, out),
        Command::Schema { file } => inspect::schema(&open(&file)?, out),
        Command::Head { file, rows } => inspect::head(&open(&file)?, rows, out),
        Command::Stats {
            file,
            columns,
            json,
        } => inspect::stats(&open(&file)?, columns, json, out),
        command @ (Command::SetCell { .. }
        | Command::InsertRows { .. }
        | Command::DeleteRows { .. }
        | Command::KeepRows { .. }
        | Command::AddColumn { .. }
        | Command::RemoveColumn { .. }
        | Command::RenameColumn { .. }
        | Command::MoveColumn { .. }) => edit::run(command, out),
        Command::Meta { command } => meta::meta(command, out),
        Command::WriterSettings(args) => meta::writer_settings(args, out),
        Command::Convert(args) => convert::convert(args, out),
        Command::Apply {
            recipe,
            file,
            output,
        } => apply::apply(&recipe, &file, &output, out),
    }
}

/// Files are always paged: commands read only what they print or write, so
/// memory stays bounded even for large files.
pub(crate) fn open(path: &std::path::Path) -> Result<Document, Error> {
    Document::open(path, OpenOptions::paged()).map_err(|e| {
        Error::Core(match e {
            veta_core::Error::Io(io) => veta_core::Error::Io(std::io::Error::new(
                io.kind(),
                format!("{}: {io}", path.display()),
            )),
            other => other,
        })
    })
}
