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
                                self.errors.push(format!("Could not filter: {e}"));
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
                    Err(e) => doc.fail_compute(&key, e),
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
                            doc.fail_compute(&c.key, "Cancelled".into());
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
                self.dialog = Some(Dialog::Format {
                    document: id,
                    dialog: FormatDialog::new(columns.clone(), *op),
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
                    Step::Filter(filter) => Dialog::Filter {
                        document: id,
                        dialog: FilterDialog::edit(columns, filter).with_nested(nested),
                        replace: Some(index),
                    },
                    Step::Sort(sort) => Dialog::Sort {
                        document: id,
                        dialog: SortDialog::new(columns, sort),
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

    /// Closes the dialog and adds `command`'s step, or replaces step
    /// `replace` with `step`. An error adding it is returned to show in the
    /// dialog, which then stays open.
    pub(crate) fn add_or_replace(
        &mut self,
        id: DocumentId,
        replace: Option<usize>,
        step: Step,
        command: Command,
    ) -> Result<(), String> {
        if let Some(index) = replace {
            self.dialog = None;
            self.change_steps(id, index, Command::ReplaceStep { index, step });
            return Ok(());
        }
        let Some(doc) = self.workbook.get_mut(id) else {
            return Ok(());
        };
        controller::execute(doc, command).map_err(|e| e.to_string())?;
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
                    let count = error
                        .split_once(": ")
                        .map_or(String::new(), |(_, rest)| format!(" ({rest})"));
                    self.errors.push(format!(
                        "Some values couldn't be converted{count}. They're kept in column \
                         {column}."
                    ));
                }
            }
            Status::Broken { step, error } => {
                let label = doc
                    .steps()
                    .get(step)
                    .map(Step::describe)
                    .unwrap_or_default();
                self.errors
                    .push(format!("Stopped at step {} ({label}): {error}", step + 1));
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
                let result = result.map(Slot::new).map_err(|e| e.to_string());
                let _ = sender.unbounded_send(Message::ComputeDone(id, target, key, result));
            }
        });
        Task::run(receiver, |message| message)
    }
}
