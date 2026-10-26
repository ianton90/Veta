//! Reading a CSV file.

use std::path::PathBuf;

use crate::error::Result;
use crate::files::convert::{CsvOptions, read_csv_data};
use crate::plan::{Effect, FileSettings, Planned, Shape, Step};

/// Reads a CSV file. CSV can't be read in parts, so it is read when the
/// step is added, lazy or not; values that don't fit their column's type
/// follow [`CsvOptions::on_error`].
#[derive(Debug)]
pub(crate) struct ReadCsv {
    pub(crate) path: PathBuf,
    pub(crate) options: CsvOptions,
}

impl Step for ReadCsv {
    fn plan(&self, _input: &Shape, _lazy: bool) -> Result<Planned> {
        let budget = crate::memory::budget();
        let (data, _report) = read_csv_data(&self.path, &self.options, budget, &mut |_| true)?;
        Ok(Planned {
            schema: data.schema(),
            effect: Effect::Data(data),
            file: Some(FileSettings::default()),
        })
    }
}
