//! Applied steps: editing them and computing their results.

use super::*;

/// Whether the step settings dialog exists for this kind of step.
pub(crate) fn step_dialog_supported(step: &Step) -> bool {
    matches!(
        step,
        Step::RenameColumn { .. }
            | Step::AddColumn { .. }
            | Step::Filter(_)
            | Step::Sort(_)
            | Step::Cast(_)
            | Step::Replace(_)
            | Step::FormatText { .. }
            | Step::Split(_)
            | Step::Merge(_)
            | Step::CombineStruct { .. }
            | Step::CollectList { .. }
            | Step::CustomColumn(_)
    )
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
                    if let (Some(doc), Some(tab)) = (
                        self.workbook.get(id),
                        self.tabs.iter_mut().find(|t| t.id == id),
                    ) {
                        match result.map(|slot| slot.take()) {
                            Ok(Some(computed)) => {
                                tab.grid.install_filter(computed, doc);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tab.grid.set_filter(None, doc);
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
                if let (Some(doc), Some(tab)) = (
                    self.workbook.get(id),
                    self.tabs.iter_mut().find(|t| t.id == id),
                ) {
                    let preview = (level < doc.evaluated_steps()).then_some(level);
                    tab.grid.set_preview(preview, doc);
                }
            }
            Message::ToggleStep(id, index) => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id)
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

    /// Opens the settings dialog of the step at `index`.
    fn edit_step(&mut self, id: DocumentId, index: usize) -> Task<Message> {
        let Some(step) = self
            .workbook
            .get(id)
            .and_then(|d| d.steps().get(index))
            .cloned()
        else {
            return Task::none();
        };
        let dialog = match &step {
            Step::RenameColumn { from, to } => ColumnDialog::rename(from.clone()).with_name(to),
            Step::AddColumn {
                name,
                data_type,
                at,
            } => ColumnDialog::add(*at).with_name(name).with_type(data_type),
            Step::Cast(cast) => {
                self.open_cast_dialog(id, Some((index, cast.clone())));
                return Task::none();
            }
            Step::Replace(r) => return self.open_replace_dialog(id, Some((index, r.clone()))),
            Step::Split(s) => return self.open_split_dialog(id, Some((index, s.clone()))),
            Step::CustomColumn(c) => {
                return self.open_custom_dialog(id, Some((index, c.clone())), String::new());
            }
            Step::Merge(m) => return self.open_merge_dialog(id, Some((index, m.clone()))),
            Step::CombineStruct { columns, name } => {
                let step = (columns.clone(), name.clone());
                return self.open_combine_dialog(id, Some((index, step)));
            }
            Step::CollectList { keys, columns } => {
                self.open_collect_dialog(id, Some((index, keys.clone(), columns.clone())));
                return Task::none();
            }
            Step::FormatText { columns, op } => {
                self.edit = None;
                self.dialog = Some(Dialog::Step {
                    document: id,
                    dialog: Box::new(FormatDialog::new(columns.clone(), *op)),
                    replace: Some(index),
                });
                return Task::none();
            }
            Step::Filter(_) | Step::Sort(_) => {
                let schema = self.workbook.get(id).and_then(|d| d.schema_at(index));
                let columns = schema
                    .as_ref()
                    .map(|schema| schema.fields().iter().map(|f| f.name().clone()).collect())
                    .unwrap_or_default();
                let nested = schema.as_deref().map(nested_columns).unwrap_or_default();
                self.edit = None;
                self.dialog = Some(match &step {
                    Step::Filter(filter) => Dialog::Step {
                        document: id,
                        dialog: Box::new(FilterDialog::edit(columns, filter).with_nested(nested)),
                        replace: Some(index),
                    },
                    Step::Sort(sort) => Dialog::Step {
                        document: id,
                        dialog: Box::new(SortDialog::new(columns, sort)),
                        replace: Some(index),
                    },
                    _ => return Task::none(),
                });
                return Task::none();
            }
            _ => return Task::none(),
        };
        let task = self.open_column_dialog(id, dialog);
        if let Some(Dialog::Column { replace, .. }) = &mut self.dialog {
            *replace = Some(index);
        }
        task
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
            // With an error column, a second step removes it if nothing
            // failed (checked on every row).
            Step::Cast(cast) if cast.options.on_error == OnError::ErrorColumn => {
                let name = cast.error_column();
                Command::AppendSteps {
                    steps: vec![Step::Cast(cast), Step::RemoveEmptyColumn { name }],
                }
            }
            step => step_command(step),
        };
        let check = matches!(command, Command::AppendSteps { .. });
        let Some(doc) = self.workbook.get_mut(id) else {
            return Ok(());
        };
        controller::execute(doc, command).map_err(|e| error_text(&e))?;
        if check {
            self.error_check = Some((id, doc.steps().len().saturating_sub(1)));
        }
        self.dialog = None;
        self.refresh(id);
        Ok(())
    }

    /// Reacts to a document's pass finishing: a failed error-column check
    /// is dropped (keeping the errors); other failures are reported.
    pub(crate) fn after_compute(&mut self, id: DocumentId) {
        let Some(doc) = self.workbook.get_mut(id) else {
            return;
        };
        let check = self.error_check.filter(|(d, _)| *d == id);
        match doc.status().clone() {
            Status::Broken { step, error } if check == Some((id, step)) => {
                self.error_check = None;
                let column = match doc.steps().get(step) {
                    Some(Step::RemoveEmptyColumn { name }) => name.clone(),
                    _ => return,
                };
                if controller::amend(doc, Command::RemoveStep { index: step }).is_ok() {
                    // The removed step's error says how many values failed.
                    let values = match error.as_ref() {
                        veta_core::Error::Invalid(invalid) => invalid.arg("values"),
                        _ => None,
                    };
                    self.errors.push(tr_with(
                        "ST_MSG_VALUES_KEPT",
                        &[("n", &values.unwrap_or("?")), ("column", &column)],
                    ));
                }
            }
            Status::Broken { step, error } => {
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
            Status::Ready if check.is_some() => self.error_check = None,
            _ => {}
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
                Target::View => self
                    .tabs
                    .iter()
                    .find(|t| t.id == c.document)
                    .and_then(|t| t.grid.filter_key()),
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
            let view = self.tabs.iter().find(|t| t.id == id);
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

/// The command adding `step`, for the steps the step dialogs make.
fn step_command(step: Step) -> Command {
    match step {
        Step::Filter(filter) => Command::Filter(filter),
        Step::Sort(sort) => Command::Sort(sort),
        Step::Cast(cast) => Command::Cast(cast),
        Step::Replace(replace) => Command::Replace(replace),
        Step::FormatText { columns, op } => Command::FormatText { columns, op },
        Step::Split(split) => Command::Split(split),
        Step::Merge(merge) => Command::Merge(merge),
        Step::CombineStruct { columns, name } => Command::CombineStruct { columns, name },
        Step::CollectList { keys, columns } => Command::CollectList { keys, columns },
        Step::CustomColumn(custom) => Command::CustomColumn(custom),
        step => unreachable!("no step dialog makes {step:?}"),
    }
}
