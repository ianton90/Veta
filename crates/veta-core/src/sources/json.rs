//! Reading a JSON file.

use std::path::PathBuf;

use super::reading;
use std::sync::Arc;

use crate::error::Result;
use crate::files::convert::read_json_data;
use crate::plan::{Context, Effect, FileSettings, Node, Planned, Step};

/// Reads a JSON file (one object per line, or an array of objects). It is
/// read when the step is added, lazy or not.
#[derive(Debug)]
pub(crate) struct ReadJson {
    pub(crate) path: PathBuf,
}

impl Step for ReadJson {
    fn plan(&self, _input: &Arc<Node>, context: &Context) -> Result<Planned> {
        let mut progress = reading(context);
        let data = read_json_data(&self.path, crate::memory::budget(), &mut progress)?;
        Ok(Planned {
            schema: data.schema(),
            effect: Effect::Data(data),
            file: Some(FileSettings::default()),
        })
    }
}
