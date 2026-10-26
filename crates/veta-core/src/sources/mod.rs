//! Source steps: they read data and replace whatever came before them. A
//! frame starts with one; a later one starts over from what it reads,
//! while the steps before it stay in the chain (a join can refer to them).

mod csv;
mod json;
mod parquet;

use std::sync::Arc;

use crate::error::Result;
use crate::plan::store::Table;
use crate::plan::{Context, Effect, FileSettings, Node, Planned, Step, Task};
use crate::source::DataSource;

pub(crate) use csv::ReadCsv;
pub(crate) use json::ReadJson;
pub(crate) use parquet::ReadParquet;

/// Reports a read's fraction done to the context's progress; `false` once
/// it is cancelled.
fn reading(context: &Context) -> impl FnMut(f32) -> bool + '_ {
    if let Some(progress) = &context.progress {
        progress.start(Task::Reading, 0);
    }
    move |fraction| match &context.progress {
        Some(progress) => progress.set_fraction(fraction),
        None => true,
    }
}

/// Data handed over in memory.
#[derive(Debug)]
pub(crate) struct Batches(pub(crate) Arc<Table>);

impl Step for Batches {
    fn plan(&self, _input: &Arc<Node>, _context: &Context) -> Result<Planned> {
        Ok(Planned {
            schema: self.0.schema(),
            effect: Effect::Data(self.0.clone()),
            file: Some(FileSettings::default()),
        })
    }
}
