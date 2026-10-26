//! Source steps: they read data and replace whatever came before them. A
//! frame starts with one; a later one starts over from what it reads,
//! while the steps before it stay in the chain (a join can refer to them).

mod csv;
mod json;
mod parquet;

use std::sync::Arc;

use crate::error::Result;
use crate::plan::store::Table;
use crate::plan::{Effect, FileSettings, Planned, Shape, Step};
use crate::source::DataSource;

pub(crate) use csv::ReadCsv;
pub(crate) use json::ReadJson;
pub(crate) use parquet::ReadParquet;

/// Data handed over in memory.
#[derive(Debug)]
pub(crate) struct Batches(pub(crate) Arc<Table>);

impl Step for Batches {
    fn plan(&self, _input: &Shape, _lazy: bool) -> Result<Planned> {
        Ok(Planned {
            schema: self.0.schema(),
            effect: Effect::Data(self.0.clone()),
            file: Some(FileSettings::default()),
        })
    }
}
