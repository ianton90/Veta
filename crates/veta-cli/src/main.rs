//! The `veta-cli` binary: headless commands on Parquet, CSV and JSON files.

use std::process::ExitCode;

/// mimalloc instead of the system allocator (decision 46). On Windows the
/// system heap hands every large block back to the OS when freed and gets
/// fresh, zeroed pages for the next one; Veta allocates and frees multi-GB
/// buffers, which made a 10 GB sort 3-4x slower. mimalloc (also used by
/// polars) keeps per-thread pools and reuses freed memory. Measure large
/// sorts and saves on Windows before changing it: Linux doesn't show this.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    veta_cli::run(veta_cli::parse())
}
