//! Reading a CSV file.

use std::path::PathBuf;

use super::reading;
use std::sync::Arc;

use crate::error::Result;
use crate::files::convert::{CsvOptions, read_csv_data};
use crate::plan::{Context, Effect, FileSettings, Node, Planned, Step};

/// Reads a CSV file. CSV can't be read in parts, so it is read when the
/// step is added, lazy or not; values that don't fit their column's type
/// follow [`CsvOptions::on_error`].
#[derive(Debug)]
pub(crate) struct ReadCsv {
    pub(crate) path: PathBuf,
    pub(crate) options: CsvOptions,
}

impl Step for ReadCsv {
    fn plan(&self, _input: &Arc<Node>, context: &Context) -> Result<Planned> {
        let budget = crate::memory::budget();
        let mut progress = reading(context);
        let (data, _report) = read_csv_data(&self.path, &self.options, budget, &mut progress)?;
        Ok(Planned {
            schema: data.schema(),
            effect: Effect::Data(data),
            file: Some(FileSettings::default()),
        })
    }
}
