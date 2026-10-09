//! Veta's command-line interface. Parses arguments with clap and runs
//! headless commands through `veta-core`.

mod edit;
mod table;

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use veta_core::arrow::datatypes::{DataType, Field};
use veta_core::display::{abbreviate, format_batch, human_bytes, type_name};
use veta_core::{Command as CoreCommand, Document, OpenOptions};

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
        output: edit::Output,
    },
    /// Insert empty rows before row AT (use the row count + 1 to append).
    InsertRows {
        file: PathBuf,
        #[arg(long)]
        at: usize,
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[command(flatten)]
        output: edit::Output,
    },
    /// Delete rows, e.g. --rows 5 --rows 10-20 or --rows 5,10-20.
    DeleteRows {
        file: PathBuf,
        #[arg(long, required = true)]
        rows: Vec<String>,
        #[command(flatten)]
        output: edit::Output,
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
        output: edit::Output,
    },
    /// Remove columns.
    RemoveColumn {
        file: PathBuf,
        #[arg(required = true)]
        names: Vec<String>,
        #[command(flatten)]
        output: edit::Output,
    },
    /// Rename a column.
    RenameColumn {
        file: PathBuf,
        from: String,
        to: String,
        #[command(flatten)]
        output: edit::Output,
    },
    /// Move a column to a position, from 1.
    MoveColumn {
        file: PathBuf,
        name: String,
        #[arg(long)]
        to: usize,
        #[command(flatten)]
        output: edit::Output,
    },
    /// Read or change key/value metadata.
    Meta {
        #[command(subcommand)]
        command: edit::MetaCommand,
    },
    /// Show writer settings, or change them with the options below.
    WriterSettings(edit::WriterArgs),
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
    parse_from(std::env::args_os())
}

/// Like [`parse`], for the given arguments (the first is the program name).
///
/// Arguments that start with a file path instead of a subcommand open the
/// GUI with those files: this is how file managers launch the app
/// (`veta C:\data\sales.parquet`).
pub fn parse_from<I, T>(args: I) -> Invocation
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if args.get(1).is_some_and(|first| is_file_argument(first)) {
        return Invocation::Gui {
            files: args[1..].iter().map(PathBuf::from).collect(),
        };
    }
    invocation(Cli::parse_from(args))
}

/// Whether `arg` is neither an option nor a subcommand name.
fn is_file_argument(arg: &OsStr) -> bool {
    let arg = arg.to_string_lossy();
    if arg.starts_with('-') || arg == "help" {
        return false;
    }
    !Cli::command()
        .get_subcommands()
        .any(|c| c.get_name() == arg || c.get_all_aliases().any(|a| a == arg))
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
        Command::Open { .. } => Err(Error::Usage("`open` starts the desktop app".into())),
        Command::Info { file } => info(&open(&file)?, out),
        Command::Schema { file } => schema(&open(&file)?, out),
        Command::Head { file, rows } => head(&open(&file)?, rows, out),
        Command::SetCell {
            file,
            row,
            column,
            value,
            null,
            output,
        } => {
            let row = edit::row_index(row)?;
            let value = if null { None } else { value };
            edit::edit(&file, &output, out, |_| {
                Ok(vec![CoreCommand::SetCell { row, column, value }])
            })
        }
        Command::InsertRows {
            file,
            at,
            count,
            output,
        } => {
            let at = edit::row_index(at)?;
            edit::edit(&file, &output, out, |_| {
                Ok(vec![CoreCommand::InsertRows { at, count }])
            })
        }
        Command::DeleteRows { file, rows, output } => {
            let rows = edit::parse_rows(&rows)?;
            edit::edit(&file, &output, out, |_| {
                Ok(vec![CoreCommand::DeleteRows { rows }])
            })
        }
        Command::AddColumn {
            file,
            name,
            data_type,
            at,
            output,
        } => {
            let data_type = edit::parse_type(&data_type)?;
            let at = at.map(edit::row_index).transpose()?;
            edit::edit(&file, &output, out, |doc| {
                Ok(vec![CoreCommand::AddColumn {
                    name,
                    data_type,
                    at: at.unwrap_or(doc.num_columns()),
                }])
            })
        }
        Command::RemoveColumn {
            file,
            names,
            output,
        } => edit::edit(&file, &output, out, |_| {
            Ok(vec![CoreCommand::RemoveColumns { names }])
        }),
        Command::RenameColumn {
            file,
            from,
            to,
            output,
        } => edit::edit(&file, &output, out, |_| {
            Ok(vec![CoreCommand::RenameColumn { from, to }])
        }),
        Command::MoveColumn {
            file,
            name,
            to,
            output,
        } => {
            let to = edit::row_index(to)?;
            edit::edit(&file, &output, out, |_| {
                Ok(vec![CoreCommand::MoveColumn { name, to }])
            })
        }
        Command::Meta { command } => edit::meta(command, out),
        Command::WriterSettings(args) => edit::writer_settings(args, out),
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

    writeln!(out)?;
    print_writer_settings(doc, out)
}

/// Prints per-column writer settings as a table.
pub(crate) fn print_writer_settings(doc: &Document, out: &mut impl Write) -> Result<(), Error> {
    let writer = doc.writer_settings();
    writeln!(
        out,
        "Writer: format {}, {} rows per row group",
        writer.format_version, writer.max_row_group_rows
    )?;
    let schema = doc.schema();
    let header = [
        "column",
        "compression",
        "encoding",
        "dictionary",
        "statistics",
        "bloom",
    ];
    let mut paths: Vec<String> = writer.columns.iter().map(|(p, _)| p.clone()).collect();
    for f in schema.fields() {
        if !paths.contains(f.name()) {
            paths.push(f.name().clone());
        }
    }
    let rows = paths
        .iter()
        .map(|path| {
            let c = writer.column(path);
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
    fn bare_file_paths_open_the_gui() {
        // File managers launch `veta <path>` without the `open` subcommand.
        assert_eq!(
            parse_from(["veta", r"C:\Users\me\data.parquet"]),
            Invocation::Gui {
                files: vec![r"C:\Users\me\data.parquet".into()]
            }
        );
        assert_eq!(
            parse_from(["veta", "a.parquet", "b.parquet"]),
            Invocation::Gui {
                files: vec!["a.parquet".into(), "b.parquet".into()]
            }
        );
        assert_eq!(
            parse_from(["veta", "schema", "a.parquet"]),
            Invocation::Cli(Command::Schema {
                file: "a.parquet".into()
            })
        );
        assert_eq!(parse_from(["veta"]), Invocation::Gui { files: vec![] });
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

    fn run_args(args: &[&str]) -> Result<String, String> {
        let Invocation::Cli(command) = parse_args(args) else {
            panic!("not a CLI command");
        };
        let mut out = Vec::new();
        execute(command, &mut out).map_err(|e| e.to_string())?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn edit_commands_change_the_file() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);
        let f = file.to_str().unwrap();

        run_args(&[
            "veta", "set-cell", f, "--row", "1", "--column", "name", "--value", "first",
        ])
        .unwrap();
        run_args(&[
            "veta", "set-cell", f, "--row", "2", "--column", "score", "--null",
        ])
        .unwrap();
        run_args(&["veta", "insert-rows", f, "--at", "1001", "--count", "2"]).unwrap();
        run_args(&["veta", "delete-rows", f, "--rows", "3-4,10"]).unwrap();
        run_args(&[
            "veta",
            "add-column",
            f,
            "note",
            "--type",
            "string",
            "--at",
            "1",
        ])
        .unwrap();
        run_args(&["veta", "rename-column", f, "flag", "ok"]).unwrap();
        run_args(&["veta", "move-column", f, "ok", "--to", "2"]).unwrap();
        run_args(&["veta", "remove-column", f, "score"]).unwrap();

        let doc = Document::open(&file, OpenOptions::default()).unwrap();
        assert_eq!(doc.num_rows(), 1000 + 2 - 3);
        let names: Vec<_> = doc
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert_eq!(names, ["note", "ok", "id", "name"]);
        let head = output(Command::Head {
            file: file.clone(),
            rows: 2,
        });
        assert!(head.contains("first"), "{head}");
    }

    #[test]
    fn edit_to_output_leaves_source_alone() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);
        let copy = dir.join("copy.parquet");
        let text = run_args(&[
            "veta",
            "delete-rows",
            file.to_str().unwrap(),
            "--rows",
            "1-500",
            "-o",
            copy.to_str().unwrap(),
        ])
        .unwrap();
        assert!(text.starts_with("Saved"));
        assert_eq!(
            Document::open(&file, OpenOptions::default())
                .unwrap()
                .num_rows(),
            1000
        );
        assert_eq!(
            Document::open(&copy, OpenOptions::default())
                .unwrap()
                .num_rows(),
            500
        );
    }

    #[test]
    fn meta_commands() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);
        let f = file.to_str().unwrap();
        run_args(&["veta", "meta", "set", f, "team", "data"]).unwrap();
        assert_eq!(
            run_args(&["veta", "meta", "get", f, "team"]).unwrap(),
            "data\n"
        );
        run_args(&["veta", "meta", "remove", f, "owner"]).unwrap();
        let list = run_args(&["veta", "meta", "list", f]).unwrap();
        assert!(
            list.contains("team = data") && !list.contains("owner"),
            "{list}"
        );
        assert!(run_args(&["veta", "meta", "get", f, "owner"]).is_err());
    }

    #[test]
    fn writer_settings_command() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);
        let f = file.to_str().unwrap();
        let shown = run_args(&["veta", "writer-settings", f]).unwrap();
        assert!(shown.contains("300 rows per row group"), "{shown}");

        run_args(&[
            "veta",
            "writer-settings",
            f,
            "--column",
            "name",
            "--compression",
            "zstd",
            "--level",
            "5",
            "--encoding",
            "delta-byte-array",
            "--dictionary",
            "off",
            "--row-group-rows",
            "100",
        ])
        .unwrap();
        let doc = Document::open(&file, OpenOptions::default()).unwrap();
        let name = doc.writer_settings().column("name");
        assert!(matches!(name.compression, veta_core::Compression::Zstd(_)));
        assert_eq!(name.encoding, veta_core::Encoding::DeltaByteArray);
        assert_eq!(doc.file_info().unwrap().row_groups.len(), 10);

        let err = run_args(&[
            "veta",
            "writer-settings",
            f,
            "--column",
            "flag",
            "--encoding",
            "delta-byte-array",
        ])
        .unwrap_err();
        assert!(err.contains("does not apply"), "{err}");
        assert!(run_args(&["veta", "writer-settings", f, "--level", "3"]).is_err());
    }

    #[test]
    fn edit_errors_are_reported() {
        let dir = TempDir::new();
        let file = dir.join("mixed.parquet");
        fixtures::mixed_settings(&file);
        let f = file.to_str().unwrap();
        let err = run_args(&[
            "veta", "set-cell", f, "--row", "1", "--column", "score", "--value", "abc",
        ])
        .unwrap_err();
        assert!(err.contains("not a valid float64"), "{err}");
        assert!(
            run_args(&[
                "veta", "set-cell", f, "--row", "0", "--column", "id", "--value", "1"
            ])
            .is_err()
        );
        assert!(run_args(&["veta", "add-column", f, "x", "--type", "colour"]).is_err());
        assert!(run_args(&["veta", "remove-column", f, "nope"]).is_err());
        // Nothing was written by the failed commands.
        assert!(
            !Document::open(&file, OpenOptions::default())
                .unwrap()
                .is_modified()
        );
        assert_eq!(
            output(Command::Head {
                file: file.clone(),
                rows: 1
            })
            .lines()
            .count(),
            5
        );
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
