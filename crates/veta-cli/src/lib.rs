//! Veta's command-line interface. Parses arguments with clap and runs
//! headless commands through `veta-core`.

mod table;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use veta_core::arrow::datatypes::{DataType, Field};
use veta_core::display::{abbreviate, format_batch, human_bytes, type_name};
use veta_core::{Document, OpenOptions};

/// Parquet viewer and editor.
///
/// Run without arguments to open the desktop app.
#[derive(Debug, Parser)]
#[command(name = "veta", version, about, long_about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// A headless command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Open files in the desktop app.
    Open {
        /// Parquet files to open.
        files: Vec<PathBuf>,
    },
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
}

/// What the `veta` binary should do for the given arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    /// Open the desktop app with these files.
    Gui { files: Vec<PathBuf> },
    /// Run a headless command.
    Cli(Command),
}

/// Parses the process arguments. Prints help/version or an argument error
/// and exits the process when clap requires it.
pub fn parse() -> Invocation {
    invocation(Cli::parse())
}

fn invocation(cli: Cli) -> Invocation {
    match cli.command {
        None => Invocation::Gui { files: Vec::new() },
        Some(Command::Open { files }) => Invocation::Gui { files },
        Some(command) => Invocation::Cli(command),
    }
}

/// Runs a headless command, printing to stdout and errors to stderr.
pub fn run(command: Command) -> ExitCode {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match execute(command, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        // Broken pipe (e.g. `veta head f | head -1`) is not an error.
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            let _ = out.flush();
            eprintln!("veta: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
enum Error {
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
        Command::Open { .. } => Err(Error::Usage("`open` starts the desktop app".into())),
        Command::Info { file } => info(&open(&file)?, out),
        Command::Schema { file } => schema(&open(&file)?, out),
        Command::Head { file, rows } => head(&open(&file)?, rows, out),
    }
}

/// CLI commands read only what they print, so files are always paged.
fn open(path: &std::path::Path) -> Result<Document, Error> {
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

fn info(doc: &Document, out: &mut impl Write) -> Result<(), Error> {
    let writer = doc.writer_settings();
    let meta = doc.metadata();
    if let Some(path) = doc.path() {
        writeln!(out, "File:        {}", path.display())?;
    }
    if let Some(info) = doc.file_info() {
        writeln!(
            out,
            "Size:        {} ({} uncompressed)",
            human_bytes(info.file_size),
            human_bytes(info.uncompressed_size())
        )?;
        writeln!(out, "Rows:        {}", doc.num_rows())?;
        writeln!(out, "Columns:     {}", doc.num_columns())?;
        writeln!(
            out,
            "Row groups:  {} (max {} rows)",
            info.row_groups.len(),
            writer.max_row_group_rows
        )?;
    }
    writeln!(out, "Format:      {}", writer.format_version)?;
    if let Some(created_by) = &meta.created_by {
        writeln!(out, "Created by:  {created_by}")?;
    }

    if !meta.key_value.is_empty() {
        writeln!(out, "\nMetadata:")?;
        for kv in &meta.key_value {
            match &kv.value {
                Some(value) => writeln!(out, "  {} = {}", kv.key, abbreviate(value, 80))?,
                None => writeln!(out, "  {}", kv.key)?,
            }
        }
    }

    if !writer.columns.is_empty() {
        writeln!(out, "\nColumns:")?;
        let header = [
            "column",
            "compression",
            "encoding",
            "dictionary",
            "statistics",
            "bloom",
        ];
        let rows = writer
            .columns
            .iter()
            .map(|(path, c)| {
                vec![
                    Some(path.clone()),
                    Some(c.compression.to_string()),
                    Some(c.encoding.to_string()),
                    Some(yes_no(c.dictionary).to_owned()),
                    Some(c.statistics.to_string()),
                    Some(yes_no(c.bloom_filter).to_owned()),
                ]
            })
            .collect::<Vec<_>>();
        table::write(out, &header.map(String::from), None, &rows, &[false; 6])?;
    }
    Ok(())
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn schema(doc: &Document, out: &mut impl Write) -> Result<(), Error> {
    fn field(out: &mut impl Write, f: &Field, depth: usize) -> std::io::Result<()> {
        let indent = "  ".repeat(depth);
        let required = if f.is_nullable() { "" } else { " (required)" };
        let ty = match f.data_type() {
            DataType::Struct(_) => "struct".to_owned(),
            other => type_name(other),
        };
        writeln!(out, "{indent}{}: {ty}{required}", f.name())?;
        if let DataType::Struct(children) = f.data_type() {
            for child in children {
                field(out, child, depth + 1)?;
            }
        }
        Ok(())
    }
    for f in doc.schema().fields() {
        field(out, f, 0)?;
    }
    Ok(())
}

fn head(doc: &Document, rows: usize, out: &mut impl Write) -> Result<(), Error> {
    let schema = doc.schema();
    let batch = doc.read(0..rows)?;
    let cells = format_batch(&batch)?;
    let names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    let types: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| type_name(f.data_type()))
        .collect();
    let right: Vec<bool> = schema
        .fields()
        .iter()
        .map(|f| veta_core::display::is_numeric(f.data_type()))
        .collect();
    table::write(out, &names, Some(&types), &cells, &right)?;
    if doc.num_rows() > rows {
        writeln!(out, "({} of {} rows)", batch.num_rows(), doc.num_rows())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use veta_testkit::{TempDir, fixtures};

    fn parse_args(args: &[&str]) -> Invocation {
        invocation(Cli::try_parse_from(args).unwrap())
    }

    fn output(command: Command) -> String {
        let mut out = Vec::new();
        execute(command, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn invocations() {
        assert_eq!(parse_args(&["veta"]), Invocation::Gui { files: vec![] });
        assert_eq!(
            parse_args(&["veta", "open", "a.parquet", "b.parquet"]),
            Invocation::Gui {
                files: vec!["a.parquet".into(), "b.parquet".into()]
            }
        );
        assert_eq!(
            parse_args(&["veta", "head", "a.parquet", "-n", "3"]),
            Invocation::Cli(Command::Head {
                file: "a.parquet".into(),
                rows: 3
            })
        );
    }

    #[test]
    fn info_shows_layout_settings_and_metadata() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);

        let text = output(Command::Info { file });
        assert!(text.contains("Rows:        1000"), "{text}");
        assert!(text.contains("Row groups:  4 (max 300 rows)"), "{text}");
        assert!(text.contains("owner = veta-tests"), "{text}");
        assert!(text.contains("delta_binary_packed"), "{text}");
        assert!(text.contains("zstd"), "{text}");
    }

    #[test]
    fn schema_lists_fields() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);

        let text = output(Command::Schema { file });
        assert_eq!(
            text,
            "id: int64 (required)\nname: string\nscore: float64\nflag: bool\n"
        );
    }

    #[test]
    fn head_prints_requested_rows() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);

        let text = output(Command::Head { file, rows: 3 });
        let lines: Vec<&str> = text.lines().collect();
        // header, types, separator, 3 rows, footer
        assert_eq!(lines.len(), 7, "{text}");
        assert!(lines[0].contains("id") && lines[0].contains("name"));
        assert!(lines[1].contains("int64"));
        assert!(lines[3].contains("null"), "row 0 has a null name: {text}");
        assert_eq!(lines[6], "(3 of 1000 rows)");
    }

    #[test]
    fn missing_file_error_names_the_file() {
        let mut out = Vec::new();
        let err = execute(
            Command::Info {
                file: "/nonexistent/x.parquet".into(),
            },
            &mut out,
        )
        .unwrap_err();
        assert!(err.to_string().contains("/nonexistent/x.parquet"));
    }
}
