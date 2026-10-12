//! The `veta-cli` binary: headless commands on Parquet, CSV and JSON files.

use std::process::ExitCode;

fn main() -> ExitCode {
    veta_cli::run(veta_cli::parse())
}
