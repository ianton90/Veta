//! Keeping or dropping rows.

use std::sync::Arc;

use crate::error::Result;
use crate::plan::{Effect, Planned, Rows, Selection, Shape, Step};

/// Keeps `rows`, or all the others when `drop`.
#[derive(Debug)]
pub(crate) struct KeepRows {
    pub(crate) rows: Rows,
    pub(crate) drop: bool,
}

impl Step for KeepRows {
    fn plan(&self, input: &Shape, _lazy: bool) -> Result<Planned> {
        let selection = Selection::new(self.rows.clone(), input.rows, self.drop)?;
        Ok(Planned {
            schema: input.schema.clone(),
            effect: Effect::Rows(Arc::new(selection)),
            file: None,
        })
    }
}
