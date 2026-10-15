//! `apply`: run a recipe on a file.

use std::io::Write;
use std::path::Path;

use veta_core::{Command, controller};

use super::*;
use crate::Error;

/// Applies a recipe's steps to `file` and saves the result.
pub fn apply(
    recipe: &Path,
    file: &Path,
    output: &Output,
    out: &mut impl Write,
) -> Result<(), Error> {
    let text = std::fs::read_to_string(recipe)?;
    let mut doc = crate::open(file)?;
    let steps = veta_core::recipe::from_json(&text, &doc.schema())?;
    let count = steps.len();
    if count > 0 {
        controller::execute(&mut doc, Command::AppendSteps { steps })?;
    }
    let stop = |doc: &veta_core::Document, step: usize, error: &dyn std::fmt::Display| {
        Error::Usage(format!(
            "stopped at step {} ({}): {error}",
            step + 1,
            doc.steps()[step].describe()
        ))
    };
    // Run full passes here so a failing step is reported by number.
    while let Some(job) = doc.compute_job() {
        let step = job.step();
        match job.run(&mut |_| true) {
            Ok(computed) => {
                doc.install(computed);
            }
            Err(e) => return Err(stop(&doc, step, &e)),
        }
    }
    if let veta_core::steps::Status::Broken { step, error } = doc.status() {
        return Err(stop(&doc, *step, error));
    }
    doc.save(output.output.clone())?;
    let target = output.output.as_deref().unwrap_or(file);
    writeln!(out, "Applied {count} steps; saved {}", target.display())?;
    Ok(())
}
