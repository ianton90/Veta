//! Reading a JSON file.

use std::path::PathBuf;

use crate::error::Result;
use crate::files::convert::read_json_data;
use crate::plan::{Effect, FileSettings, Planned, Shape, Step};

/// Reads a JSON file (one object per line, or an array of objects). It is
/// read when the step is added, lazy or not.
#[derive(Debug)]
pub(crate) struct ReadJson {
    pub(crate) path: PathBuf,
}

impl Step for ReadJson {
    fn plan(&self, _input: &Shape, _lazy: bool) -> Result<Planned> {
        let data = read_json_data(&self.path, crate::memory::budget(), &mut |_| true)?;
        Ok(Planned {
            schema: data.schema(),
            effect: Effect::Data(data),
            file: Some(FileSettings::default()),
        })
    }
}
