//! The `veta-cli` binary: headless commands on Parquet, CSV and JSON files.

use std::process::ExitCode;

/// The memory allocator polars also uses: freed memory is reused instead of
/// handed back to the system and fetched again, which made large sorts slow
/// (decision 46).
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    veta_cli::run(veta_cli::parse())
}
