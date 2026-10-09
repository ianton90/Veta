//! Writes the test fixtures to a directory, for trying the app by hand:
//!
//! ```sh
//! cargo run -p veta-testkit --example fixtures -- /tmp/veta-fixtures
//! ```

use std::path::PathBuf;

use veta_testkit::fixtures;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures".into()));
    std::fs::create_dir_all(&dir).expect("create output directory");
    fixtures::all_types(&dir.join("all_types.parquet"));
    fixtures::mixed_settings(&dir.join("mixed_settings.parquet"));
    fixtures::large(&dir.join("large.parquet"), 5_000_000, 500_000);
    println!("Wrote fixtures to {}", dir.display());
}
