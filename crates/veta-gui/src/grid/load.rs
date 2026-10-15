//! Reading rows: the window of formatted rows, background reads and the
//! temporary filter view.

use super::*;

/// Rows to read off the UI thread (see [`GridView::take_read`]).
#[derive(Debug, Clone)]
pub struct RowRead {
    pub id: u64,
    pub(super) pipeline: Pipeline,
    /// Steps to read after; `None` for all (a filter view).
    pub(super) level: Option<usize>,
    pub(super) range: Range<usize>,
}

impl RowRead {
    /// Reads the rows; slow for paged documents, so run it on a thread.
    pub fn run(&self) -> Result<RecordBatch, String> {
        let result = match self.level {
            Some(level) => self.pipeline.read_at(level, self.range.clone()),
            None => self.pipeline.read(self.range.clone()),
        };
        result.map_err(|e| e.to_string())
    }
}

/// The document's output with a filter applied, computed in the background.
#[derive(Debug, Clone)]
pub(super) struct FilterView {
    pub(super) filter: Filter,
    pub(super) pipeline: Pipeline,
}

impl FilterView {
    fn ready(&self) -> bool {
        self.pipeline.status() == &Status::Ready
    }
}

impl GridView {
    /// Number of steps shown, if previewing an earlier step.
    pub fn preview(&self) -> Option<usize> {
        self.at
    }

    /// Shows the data after `at` steps (`None`: after all evaluated steps).
    /// Clears the filter, if any.
    pub fn set_preview(&mut self, at: Option<usize>, document: &Document) {
        // Other rows show after another step or without the filter.
        if at != self.at || self.filter.is_some() {
            self.open_rows.clear();
        }
        self.at = at;
        self.filter = None;
        self.refresh(document);
    }

    /// The temporary filter, if one is on.
    pub fn filter(&self) -> Option<&Filter> {
        self.filter.as_ref().map(|f| &f.filter)
    }

    /// Shows the document filtered by `view` (a filter and the document's
    /// pipeline with it applied), or unfiltered.
    pub fn set_filter(&mut self, view: Option<(Filter, Pipeline)>, document: &Document) {
        self.at = None;
        self.filter = view.map(|(filter, pipeline)| FilterView { filter, pipeline });
        self.open_rows.clear();
        self.refresh(document);
    }

    /// The filter's pass, while it still needs computing.
    pub fn filter_job(&self) -> Option<ComputeJob> {
        self.filter.as_ref()?.pipeline.compute_job()
    }

    pub fn filter_key(&self) -> Option<Vec<u64>> {
        self.filter.as_ref()?.pipeline.compute_key()
    }

    /// Stores the filter's computed pass; returns whether it applied.
    pub fn install_filter(&mut self, computed: Computed, document: &Document) -> bool {
        let Some(view) = &mut self.filter else {
            return false;
        };
        if !view.pipeline.install(computed) {
            return false;
        }
        self.open_rows.clear();
        self.refresh(document);
        true
    }

    /// Rows shown and rows in the document, while a filter is on and
    /// computed.
    pub fn filter_counts(&self, document: &Document) -> Option<(usize, usize)> {
        let view = self.filter.as_ref().filter(|f| f.ready())?;
        Some((view.pipeline.num_rows(), document.num_rows()))
    }

    /// The computed filter view, if any.
    pub(super) fn view(&self) -> Option<&Pipeline> {
        self.filter
            .as_ref()
            .filter(|f| f.ready())
            .map(|f| &f.pipeline)
    }

    /// The step count whose output is shown.
    pub(super) fn level(&self, document: &Document) -> usize {
        let evaluated = document.evaluated_steps();
        self.at.map_or(evaluated, |at| at.min(evaluated))
    }

    /// Re-reads the document after it changed: columns (keeping the widths
    /// of columns that still exist), row count, selection and visible rows.
    pub fn refresh(&mut self, document: &Document) {
        let old: Vec<Column> = std::mem::take(&mut self.columns);
        let level = self.level(document);
        let schema = match self.view() {
            Some(view) => view.schema(),
            None => document
                .schema_at(level)
                .unwrap_or_else(|| document.schema()),
        };
        let mut fresh = Vec::new();
        self.columns = display_columns(&schema, &self.expanded);
        for (i, column) in self.columns.iter_mut().enumerate() {
            // A column whose type changed is fitted again.
            match old
                .iter()
                .find(|c| c.key == column.key && c.type_name == column.type_name)
            {
                Some(kept) => column.width = kept.width,
                None => {
                    column.width = MIN_FITTED_WIDTH;
                    fresh.push(i);
                }
            }
        }
        let num_rows = match self.view() {
            Some(view) => view.num_rows(),
            None => document.num_rows_at(level).unwrap_or(0),
        };
        // Row numbers mean other rows now.
        if num_rows != self.num_rows {
            self.open_rows.clear();
        }
        self.num_rows = num_rows;
        let before = self.selected;
        self.selected = self.selected.and_then(|(r, c)| {
            (self.num_rows > 0 && !self.columns.is_empty())
                .then(|| (r.min(self.num_rows - 1), c.min(self.columns.len() - 1)))
        });
        if self.selected != before {
            self.item = None;
        }
        let n = self.columns.len();
        self.cols = self
            .cols
            .and_then(|(a, b)| (n > 0).then(|| (a.min(n - 1), b.min(n - 1))));
        self.rows = self.rows.and_then(|(a, b)| {
            (self.num_rows > 0).then(|| (a.min(self.num_rows - 1), b.min(self.num_rows - 1)))
        });
        self.set_scroll(self.first_row, self.scroll_x);
        self.load(document, true);
        for i in fresh {
            self.fit_column(i);
        }
    }

    /// Reads rows in the background while scrolling (for paged documents).
    pub fn set_background(&mut self, on: bool) {
        self.background = on;
    }

    /// Makes sure `cells` covers the rows in view, reading a margin of one
    /// screen above and below so small scrolls don't hit the document.
    pub(super) fn load(&mut self, document: &Document, force: bool) {
        let needed = self.first_row..(self.first_row + self.visible_rows + 1).min(self.num_rows);
        let window_end = self.window_start + self.cells.len();
        if !force && needed.start >= self.window_start && needed.end <= window_end {
            return;
        }
        let start = self.first_row.saturating_sub(self.visible_rows);
        let end = (self.first_row + 2 * self.visible_rows + 1).min(self.num_rows);
        // Scrolling in the background mode reads off the UI thread; a read
        // already on its way for these rows is waited for.
        if self.background && !force {
            if let Some((_, range)) = &self.reading
                && needed.start >= range.start
                && needed.end <= range.end
            {
                return;
            }
            self.next_read += 1;
            let (pipeline, level) = match self.view() {
                Some(view) => (view.clone(), None),
                None => (document.reader(), Some(self.level(document))),
            };
            self.wanted = Some(RowRead {
                id: self.next_read,
                pipeline,
                level,
                range: start..end,
            });
            self.reading = Some((self.next_read, start..end));
            return;
        }
        // A synchronous read replaces any read on its way.
        self.reading = None;
        self.wanted = None;
        let batch = match self.view() {
            Some(view) => view.read(start..end),
            None => document.read_at(self.level(document), start..end),
        };
        self.install(start, batch);
    }

    /// The read the app should run in the background, if one is wanted.
    pub fn take_read(&mut self) -> Option<RowRead> {
        self.wanted.take()
    }

    /// Uses the rows read by [`RowRead`] `id`, unless a newer read replaced
    /// it; then makes sure the rows now in view are loaded.
    pub fn install_read(
        &mut self,
        id: u64,
        result: Result<RecordBatch, String>,
        document: &Document,
    ) {
        let Some((current, range)) = self.reading.clone() else {
            return;
        };
        if current != id {
            return;
        }
        self.reading = None;
        let result = result.map_err(veta_core::Error::InvalidCommand);
        self.install(range.start, result);
        self.load(document, false);
    }

    /// Makes `batch` the loaded rows, starting at row `start`.
    fn install(&mut self, start: usize, batch: veta_core::Result<RecordBatch>) {
        let end = start + batch.as_ref().map_or(0, RecordBatch::num_rows);
        let formatted = batch.and_then(|batch| {
            let batch = self.display_batch(batch)?;
            let mut cells = format_batch(&batch)?;
            let mut details = std::collections::HashMap::new();
            for (i, column) in batch.columns().iter().enumerate() {
                if !has_items(column.data_type()) {
                    continue;
                }
                for (row, cells) in cells.iter_mut().enumerate() {
                    if let Some(summary) = veta_core::display::items_summary(column.as_ref(), row) {
                        cells[i] = Some(summary);
                    }
                }
            }
            for &row in self.open_rows.range(start..end) {
                let local = row - start;
                let columns = batch
                    .columns()
                    .iter()
                    .map(|column| {
                        let array = column.as_ref();
                        let kind = match array.data_type() {
                            DataType::List(_) | DataType::LargeList(_) => ItemsKind::List,
                            DataType::Struct(_) => ItemsKind::Struct,
                            t if has_items(t) => ItemsKind::Map,
                            _ => ItemsKind::None,
                        };
                        if kind == ItemsKind::Struct {
                            let items =
                                veta_core::display::struct_fields(array, local).unwrap_or_default();
                            return Items {
                                items,
                                more: 0,
                                kind,
                            };
                        }
                        let items = veta_core::display::list_items(array, local, MAX_ITEMS)
                            .unwrap_or_default();
                        let more = veta_core::display::item_count(array, local) - items.len();
                        Items { items, more, kind }
                    })
                    .collect();
                details.insert(row, columns);
            }
            Ok((cells, nested_json(&batch)?, details))
        });
        match formatted {
            Ok((cells, json, details)) => {
                self.window_start = start;
                self.cells = cells;
                self.json = json;
                self.details = details;
                self.error = None;
            }
            Err(e) => {
                self.cells.clear();
                self.json.clear();
                self.details.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    /// `batch` (document columns) as the columns on screen.
    fn display_batch(&self, batch: RecordBatch) -> veta_core::Result<RecordBatch> {
        if self.columns.iter().all(|c| c.path().is_empty()) {
            return Ok(batch);
        }
        let mut fields = Vec::new();
        let mut arrays = Vec::new();
        for (i, column) in self.columns.iter().enumerate() {
            let Some(array) = batch
                .columns()
                .get(column.source)
                .and_then(|a| veta_core::display::field_array(a, column.path()))
            else {
                // Out of step with the document; the next refresh fixes it.
                return Ok(batch);
            };
            fields.push(Field::new(format!("c{i}"), array.data_type().clone(), true));
            arrays.push(array);
        }
        let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
        Ok(RecordBatch::try_new_with_options(
            std::sync::Arc::new(Schema::new(fields)),
            arrays,
            &options,
        )?)
    }
}

/// JSON text of the nested columns of `batch`: `result[row][column]` is
/// `None` for columns that aren't nested.
fn nested_json(
    batch: &veta_core::arrow::record_batch::RecordBatch,
) -> veta_core::Result<Vec<Vec<Option<Option<String>>>>> {
    let nested: Vec<bool> = batch
        .columns()
        .iter()
        .map(|c| c.data_type().is_nested())
        .collect();
    if !nested.contains(&true) {
        return Ok(Vec::new());
    }
    (0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .zip(&nested)
                .map(|(column, &is_nested)| {
                    if is_nested {
                        Ok(Some(veta_core::display::value_json(column.as_ref(), row)?))
                    } else {
                        Ok(None)
                    }
                })
                .collect()
        })
        .collect()
}
