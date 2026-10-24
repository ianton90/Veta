//! Applied steps: editing them and computing their results.

use super::*;

/// Opens a step's settings dialog: given the document and the step's index.
type StepEditor = Box<dyn FnOnce(&mut App, DocumentId, usize) -> Task<Message>>;

/// How to open the settings dialog of a step, for the kinds that have one.
/// Every kind is listed, so a new one must be decided on here.
fn step_editor(step: &Step) -> Option<StepEditor> {
    let editor: StepEditor = match step {
        Step::AddColumn {
            name,
            data_type,
            at: column,
        } => {
            let dialog = ColumnDialog::add(*column)
                .with_name(name)
                .with_type(data_type);
            Box::new(move |app, id, at| app.open_column_dialog(id, dialog, Some(at)))
        }
        Step::RenameColumn { from, to } => {
            let dialog = ColumnDialog::rename(from.clone()).with_name(to);
            Box::new(move |app, id, at| app.open_column_dialog(id, dialog, Some(at)))
        }
        Step::AddField {
            column,
            path,
            name,
            data_type,
        } => {
            let dialog = ColumnDialog::add_field(column.clone(), path.clone())
                .with_name(name)
                .with_type(data_type);
            Box::new(move |app, id, at| app.open_column_dialog(id, dialog, Some(at)))
        }
        Step::Filter(f) => {
            let f = f.clone();
            Box::new(move |app, id, at| app.open_filter_dialog(id, Some((at, f))))
        }
        Step::Sort(s) => {
            let s = s.clone();
            Box::new(move |app, id, at| app.open_sort_dialog(id, Some((at, s))))
        }
        Step::Cast(c) => {
            let c = c.clone();
            Box::new(move |app, id, at| app.open_cast_dialog(id, Some((at, c))))
        }
        Step::Replace(r) => {
            let r = r.clone();
            Box::new(move |app, id, at| app.open_replace_dialog(id, Some((at, r))))
        }
        Step::FormatText { columns, op } => {
            let (columns, op) = (columns.clone(), *op);
            Box::new(move |app, id, at| app.open_format_dialog(id, Some((at, columns, op))))
        }
        Step::Split(s) => {
            let s = s.clone();
            Box::new(move |app, id, at| app.open_split_dialog(id, Some((at, s))))
        }
        Step::Merge(m) => {
            let m = m.clone();
            Box::new(move |app, id, at| app.open_merge_dialog(id, Some((at, m))))
        }
        Step::CustomColumn(c) => {
            let c = c.clone();
            Box::new(move |app, id, at| app.open_custom_dialog(id, Some((at, c)), String::new()))
        }
        Step::CollectList { keys, columns } => {
            let (keys, columns) = (keys.clone(), columns.clone());
            Box::new(move |app, id, at| app.open_collect_dialog(id, Some((at, keys, columns))))
        }
        Step::CombineStruct { columns, name } => {
            let step = (columns.clone(), name.clone());
            Box::new(move |app, id, at| app.open_combine_dialog(id, Some((at, step))))
        }
        Step::EditCells(_)
        | Step::InsertRows { .. }
        | Step::DeleteRows { .. }
        | Step::RemoveColumns { .. }
        | Step::KeepColumns { .. }
        | Step::MoveColumn { .. }
        | Step::Fill(_)
        | Step::ExpandStruct { .. }
        | Step::ExplodeList { .. }
        | Step::RemoveField { .. }
        | Step::RemoveEmptyColumn { .. } => return None,
    };
    Some(editor)
}

/// Whether the step settings dialog exists for this kind of step.
pub(crate) fn step_dialog_supported(step: &Step) -> bool {
    step_editor(step).is_some()
}

impl App {
    /// Messages about applied steps and their computation.
    pub(crate) fn steps_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Step(message) => {
                if let Some(Dialog::Step {
                    document,
                    dialog,
                    replace,
                }) = &mut self.dialog
                {
                    let (id, replace) = (*document, *replace);
                    let cancel = dialog.cancels(&message);
                    if let Some(step) = dialog.update(message) {
                        if let Err(e) = self.add_or_replace(id, replace, step) {
                            let unshown = match &mut self.dialog {
                                Some(Dialog::Step { dialog, .. }) => dialog.set_error(e),
                                _ => Some(e),
                            };
                            self.errors.extend(unshown);
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::ComputeProgress(id, target, key, progress) => {
                if let Some(c) = &mut self.computing
                    && c.document == id
                    && c.target == target
                    && c.key == key
                {
                    c.progress = progress;
                }
            }
            Message::ComputeDone(id, target, key, result) => {
                if self
                    .computing
                    .as_ref()
                    .is_some_and(|c| c.document == id && c.target == target && c.key == key)
                {
                    self.computing = None;
                }
                if target == Target::View {
                    if self.tab(id).is_some() {
                        match result.map(|slot| slot.take()) {
                            Ok(Some(computed)) => {
                                self.grid_do(id, |grid, doc| {
                                    grid.install_filter(computed, doc);
                                });
                            }
                            Ok(None) => {}
                            Err(e) => {
                                self.grid_do(id, |grid, doc| grid.set_filter(None, doc));
                                let error = crate::i18n::errors::error_text(&e);
                                self.errors
                                    .push(tr_with("ST_MSG_FILTER_FAILED", &[("error", &error)]));
                            }
                        }
                    }
                    return Task::none();
                }
                let Some(doc) = self.workbook.get_mut(id) else {
                    return Task::none();
                };
                let changed = match result.map(|slot| slot.take()) {
                    Ok(Some(computed)) => doc.install(computed),
                    Ok(None) => false,
                    Err(e) => doc.fail_compute(&key, std::sync::Arc::new(e)),
                };
                if changed {
                    self.after_compute(id);
                    self.refresh(id);
                }
            }
            Message::CancelCompute => {
                if let Some(c) = self.computing.take() {
                    c.cancel.store(true, Ordering::Relaxed);
                    if c.target == Target::View {
                        self.clear_filter(c.document);
                    } else if let Some(doc) = self.workbook.get_mut(c.document) {
                        if !controller::undo(doc) {
                            doc.fail_compute(
                                &c.key,
                                std::sync::Arc::new(veta_core::Error::Cancelled),
                            );
                        }
                        self.refresh(c.document);
                    }
                }
            }
            Message::PreviewStep(id, level) => {
                self.commit_edit_or_drop();
                self.grid_do(id, |grid, doc| {
                    let preview = (level < doc.evaluated_steps()).then_some(level);
                    grid.set_preview(preview, doc);
                });
            }
            Message::ToggleStep(id, index) => {
                if let Some(tab) = self.tab_mut(id)
                    && !tab.expanded_steps.remove(&index)
                {
                    tab.expanded_steps.insert(index);
                }
            }
            Message::RemoveStep(_) | Message::MoveStep(..) | Message::EditStep(_)
                if self.active.is_some_and(|id| !self.steps_changeable(id)) =>
            {
                if let Some(id) = self.active {
                    self.not_editable(id);
                }
            }
            Message::RemoveStep(index) => {
                if let Some(id) = self.active {
                    self.change_steps(id, index, Command::RemoveStep { index });
                }
            }
            Message::MoveStep(from, to) => {
                if let Some(id) = self.active {
                    self.change_steps(id, from.min(to), Command::MoveStep { from, to });
                }
            }
            Message::EditStep(index) => {
                if let Some(id) = self.active {
                    return self.edit_step(id, index);
                }
            }
            Message::ConfirmStep(yes) => {
                if let Some(Dialog::ConfirmStep { document, command }) = self.dialog.take()
                    && yes
                {
                    self.execute(document, command);
                }
            }
            _ => unreachable!("not a steps message"),
        }
        Task::none()
    }

    /// Opens a dialog that adds a step, or edits the step at `replace`.
    pub(crate) fn open_step_dialog(
        &mut self,
        id: DocumentId,
        dialog: impl StepDialog + 'static,
        replace: Option<usize>,
    ) {
        self.edit = None;
        self.dialog = Some(Dialog::Step {
            document: id,
            dialog: Box::new(dialog),
            replace,
        });
    }

    /// Opens the settings dialog of the step at `index`, if its kind has
    /// one.
    fn edit_step(&mut self, id: DocumentId, index: usize) -> Task<Message> {
        let editor = self
            .workbook
            .get(id)
            .and_then(|d| d.steps().get(index))
            .and_then(step_editor);
        match editor {
            Some(open) => open(self, id, index),
            None => Task::none(),
        }
    }

    /// Runs a step-list command, first asking if later steps refer to rows
    /// by position (they may then apply to different rows).
    pub(crate) fn change_steps(&mut self, id: DocumentId, from: usize, command: Command) {
        let positional_after = self
            .workbook
            .get(id)
            .is_some_and(|d| d.steps().iter().skip(from + 1).any(Step::is_positional));
        if positional_after {
            self.dialog = Some(Dialog::ConfirmStep {
                document: id,
                command,
            });
        } else {
            self.execute(id, command);
        }
    }

    /// Closes the step dialog and adds `step`, or replaces step `replace`
    /// with it. A new filter becomes the temporary filter view instead. An
    /// error adding the step is returned, and the dialog stays open.
    pub(crate) fn add_or_replace(
        &mut self,
        id: DocumentId,
        replace: Option<usize>,
        step: Step,
    ) -> Result<(), String> {
        if let Some(index) = replace {
            self.dialog = None;
            self.change_steps(id, index, Command::ReplaceStep { index, step });
            return Ok(());
        }
        let command = match step {
            Step::Filter(filter) => {
                self.set_view_filter(id, filter)?;
                self.dialog = None;
                return Ok(());
            }
            step => Command::AddStep(step),
        };
        let added_column = match &command {
            Command::AddStep(Step::AddColumn { at, .. }) => Some(*at),
            _ => None,
        };
        let Some(doc) = self.workbook.get_mut(id) else {
            return Ok(());
        };
        controller::execute(doc, command).map_err(|e| error_text(&e))?;
        self.dialog = None;
        self.refresh(id);
        if let Some(at) = added_column {
            self.select_columns(id, at..at + 1);
        }
        Ok(())
    }

    /// Reacts to a document's pass finishing: a failed error-column check
    /// is dropped (keeping the errors); other failures are reported.
    pub(crate) fn after_compute(&mut self, id: DocumentId) {
        let Some(doc) = self.workbook.get_mut(id) else {
            return;
        };
        if let Some(kept) = controller::settle(doc) {
            let values = kept.values.unwrap_or_else(|| "?".into());
            self.errors.push(tr_with(
                "ST_MSG_VALUES_KEPT",
                &[("n", &values), ("column", &kept.column)],
            ));
            return;
        }
        if let Status::Broken { step, error } = doc.status().clone() {
            let label = doc
                .steps()
                .get(step)
                .map(crate::i18n::model::step_name)
                .unwrap_or_default();
            self.errors.push(tr_with(
                "ST_MSG_STOPPED_AT_STEP",
                &[
                    ("n", &(step + 1)),
                    ("step", &label),
                    ("error", &crate::i18n::errors::error_text(&error)),
                ],
            ));
        }
    }

    /// Starts the next pending full pass if none is running, and stops a
    /// running one whose result would no longer apply.
    pub(crate) fn schedule_compute(&mut self) -> Task<Message> {
        if let Some(c) = &self.computing {
            let current = match c.target {
                Target::Document => self
                    .workbook
                    .get(c.document)
                    .and_then(Document::compute_key),
                Target::View => self.tab(c.document).and_then(|t| t.grid.filter_key()),
            };
            if current.as_ref() != Some(&c.key) {
                c.cancel.store(true, Ordering::Relaxed);
                self.computing = None;
            }
        }
        if self.computing.is_some() {
            return Task::none();
        }
        // The active document first.
        let mut order: Vec<DocumentId> = self.tabs.iter().map(|t| t.id).collect();
        order.sort_by_key(|id| Some(*id) != self.active);
        for id in order {
            if let Some(job) = self.workbook.get(id).and_then(Document::compute_job) {
                return self.start_compute(id, Target::Document, job);
            }
            let view = self.tab(id);
            if let Some(job) = view.and_then(|t| t.grid.filter_job()) {
                return self.start_compute(id, Target::View, job);
            }
        }
        Task::none()
    }

    fn start_compute(&mut self, id: DocumentId, target: Target, job: ComputeJob) -> Task<Message> {
        let key = job.key().to_vec();
        let cancel = Arc::new(AtomicBool::new(false));
        self.computing = Some(Computing {
            document: id,
            target,
            key: key.clone(),
            step: job.step(),
            progress: 0.0,
            cancel: cancel.clone(),
        });
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut reported = 0.0;
            let result = job.run(&mut |progress| {
                if progress - reported >= 0.01 {
                    reported = progress;
                    let _ = sender.unbounded_send(Message::ComputeProgress(
                        id,
                        target,
                        key.clone(),
                        progress,
                    ));
                }
                !cancel.load(Ordering::Relaxed)
            });
            if !cancel.load(Ordering::Relaxed) {
                let result = result.map(Slot::new);
                let _ = sender.unbounded_send(Message::ComputeDone(id, target, key, result));
            }
        });
        Task::run(receiver, |message| message)
    }
}
