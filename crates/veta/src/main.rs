//! The `veta` binary: the desktop app. Command-line use is `veta-cli`.

// A Windows GUI app: no console window behind it, in every build.
#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Parquet viewer and editor (desktop app). For command-line use, run
/// `veta-cli`.
#[derive(Debug, Parser)]
#[command(name = "veta", version = veta_core::version::LONG)]
struct Args {
    /// Files to open.
    files: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match veta_gui::run(args.files) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("veta: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn arguments_are_files() {
        Args::command().debug_assert();
        let args = Args::try_parse_from(["veta", r"C:\Users\me\data.parquet", "b.parquet"]);
        assert_eq!(
            args.map(|a| a.files).ok(),
            Some(vec![r"C:\Users\me\data.parquet".into(), "b.parquet".into()])
        );
        // Subcommand names are just file names now.
        let args = Args::try_parse_from(["veta", "info"]);
        assert_eq!(args.map(|a| a.files).ok(), Some(vec!["info".into()]));
    }
}
