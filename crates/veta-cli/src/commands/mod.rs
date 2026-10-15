//! The subcommands, by group, and helpers they share.

pub mod apply;
pub mod convert;
pub mod edit;
pub mod inspect;
pub mod meta;
#[cfg(test)]
mod tests;

use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use clap::Args;
use veta_core::display::parse_type_name;
use veta_core::{Command, Document, controller};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct Output {
    /// Write the result here instead of changing the file in place.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

/// Opens `file`, runs the commands `build` returns, and saves.
pub fn edit(
    file: &Path,
    output: &Output,
    out: &mut impl Write,
    build: impl FnOnce(&Document) -> Result<Vec<Command>, Error>,
) -> Result<(), Error> {
    let mut doc = crate::open(file)?;
    for command in build(&doc)? {
        controller::execute(&mut doc, command)?;
    }
    doc.save(output.output.clone())?;
    let target = output.output.as_deref().unwrap_or(file);
    writeln!(out, "Saved {}", target.display())?;
    Ok(())
}

/// Converts a 1-based row number to an index.
pub fn row_index(row: usize) -> Result<usize, Error> {
    row.checked_sub(1)
        .ok_or_else(|| Error::Usage("row numbers start at 1".into()))
}

/// Parses `5`, `10-20` (inclusive, 1-based) into 0-based ranges.
pub fn parse_rows(specs: &[String]) -> Result<Vec<Range<usize>>, Error> {
    specs
        .iter()
        .flat_map(|s| s.split(','))
        .map(|part| {
            let part = part.trim();
            let bad = || Error::Usage(format!("invalid row range {part:?}; use e.g. 5 or 10-20"));
            let (first, last) = match part.split_once('-') {
                Some((a, b)) => (
                    a.trim().parse().map_err(|_| bad())?,
                    b.trim().parse().map_err(|_| bad())?,
                ),
                None => {
                    let n = part.parse().map_err(|_| bad())?;
                    (n, n)
                }
            };
            if first == 0 || last < first {
                return Err(bad());
            }
            Ok(first - 1..last)
        })
        .collect()
}

pub fn parse_type(name: &str) -> Result<veta_core::arrow::datatypes::DataType, Error> {
    parse_type_name(name).ok_or_else(|| {
        Error::Usage(format!(
            "unknown type {name:?}; use e.g. string, int64, float64, bool, date, timestamp[ms], decimal(10, 2)"
        ))
    })
}
