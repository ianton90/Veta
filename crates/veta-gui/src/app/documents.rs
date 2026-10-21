//! Opening, saving, closing, importing and exporting files, and recipes.

use super::*;

pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether an error message is the core's "cancelled".
pub(crate) fn is_cancelled(error: &str) -> bool {
    error == error_text(&veta_core::Error::Cancelled)
}

/// Each top-level column with its metadata: as edited in Veta, otherwise
/// the column's own (sorted by key).
pub(crate) fn column_metadata(doc: &Document) -> Vec<(String, Vec<veta_core::model::KeyValue>)> {
    doc.schema()
        .fields()
        .iter()
        .map(|field| {
            let entries = match doc.metadata().column(field.name()) {
                Some(entries) => entries.to_vec(),
                None => {
                    let mut entries: Vec<_> = field
                        .metadata()
                        .iter()
                        .map(|(k, v)| veta_core::model::KeyValue {
                            key: k.clone(),
                            value: Some(v.clone()),
                        })
                        .collect();
                    entries.sort_by(|a, b| a.key.cmp(&b.key));
                    entries
                }
            };
            (field.name().clone(), entries)
        })
        .collect()
}

/// The file kind an export format writes.
pub(crate) fn export_kind(choice: dialogs::ExportChoice) -> FileKind {
    match choice {
        dialogs::ExportChoice::Csv => FileKind::Csv,
        dialogs::ExportChoice::JsonLines => FileKind::JsonLines,
        dialogs::ExportChoice::JsonArray => FileKind::Json,
    }
}

/// Runs blocking work on its own thread so the UI stays responsive.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.await.expect("worker thread panicked")
}

impl App {
    /// Messages about opening, saving, closing, importing and exporting files.
    pub(crate) fn documents_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenDialog => return self.perform(Action::Open),
            Message::FilesPicked(files) => return self.open(files),
            Message::Remote(files) => {
                self.queued_files.extend(files);
                return window::latest().and_then(window::gain_focus);
            }
            Message::Opened(path, result) => {
                self.opening.retain(|p| p != &path);
                match result.map(|loaded| loaded.take()) {
                    Ok(Some(document)) => self.add_document(&path, document),
                    Ok(None) => {}
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => self.errors.push(tr_with(
                        "ST_MSG_OPEN_FAILED",
                        &[("path", &path.display()), ("error", &e)],
                    )),
                }
            }
            Message::Imported(path, result) => {
                self.opening.retain(|p| p != &path);
                match result.map(|slot| slot.take()) {
                    Ok(Some((document, report))) => {
                        self.add_document(&path, document);
                        if let Some(summary) = crate::i18n::model::import_report(&report) {
                            self.errors.push(summary);
                        }
                    }
                    Ok(None) => {}
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => self.errors.push(tr_with(
                        "ST_MSG_IMPORT_FAILED",
                        &[("path", &path.display()), ("error", &e)],
                    )),
                }
            }
            Message::SaveTo(id, Some(path)) => return self.start_save(id, Some(path)),
            Message::SaveTo(_, None) => {}
            Message::Saved(id, result) => {
                self.saving = None;
                match result.map(|slot| slot.take()) {
                    Ok(Some(saved)) => {
                        if let Some(doc) = self.workbook.get_mut(id) {
                            doc.finish_save(saved);
                            if let Some(path) = doc.path().map(Path::to_path_buf) {
                                self.config.add_recent(&path);
                                self.save_config();
                            }
                        }
                        self.refresh(id);
                        if self
                            .closing
                            .as_ref()
                            .is_some_and(|c| c.queue.first() == Some(&id))
                        {
                            return self.continue_closing();
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.errors
                            .push(tr_with("ST_MSG_SAVE_FAILED", &[("error", &e)]));
                        self.closing = None;
                    }
                }
            }
            Message::SelectTab(id) => self.active = Some(id),
            Message::CloseTab(id) => return self.request_close(vec![id], false),
            Message::CloseRequested => {
                let all = self.tabs.iter().map(|t| t.id).collect();
                return self.request_close(all, true);
            }
            Message::CloseAnswer(answer) => return self.answer_close(answer),
            Message::ImportPicked(Some(path)) => return self.open(vec![path]),
            Message::ImportPicked(None) => {}
            Message::Import(message) => {
                if let Some(Dialog::Import(dialog)) = &mut self.dialog {
                    let cancel = matches!(message, ImportMessage::Cancel);
                    if let Some(options) = dialog.update(message) {
                        let path = dialog.path.clone();
                        self.dialog = None;
                        let budget = veta_core::memory::budget();
                        self.opening.push(path.clone());
                        let worker_path = path.clone();
                        return self.start_transfer(
                            Some(path.clone()),
                            tr_with("ST_MSG_IMPORTING", &[("name", &file_label(&path))]),
                            move |progress| {
                                veta_core::convert::import_csv(
                                    &worker_path,
                                    &options,
                                    budget,
                                    progress,
                                )
                                .map(Slot::new)
                                .map_err(|e| error_text(&e))
                            },
                            move |result| Message::Imported(path.clone(), result),
                        );
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Export(message) => {
                if let Some(Dialog::Export { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, ExportMessage::Cancel);
                    let submitted = dialog.update(message).is_some();
                    let kind = export_kind(dialog.choice);
                    if submitted {
                        let doc = self.workbook.get(id);
                        let name = doc
                            .map(|d| {
                                files::suggested_name(&d.title_or(&tr("ST_DOC_UNTITLED")), kind)
                            })
                            .unwrap_or_default();
                        let dir = doc
                            .and_then(Document::path)
                            .and_then(Path::parent)
                            .map(Path::to_path_buf);
                        let save = files::save(tr("ST_DLG_EXPORT_TITLE"), kind, name, dir);
                        return Task::perform(save, move |p| Message::ExportTo(id, p));
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::ExportTo(id, Some(path)) => {
                let Some(Dialog::Export { dialog, .. }) = self.dialog.take() else {
                    return Task::none();
                };
                let format = dialog.format();
                let output = match self.workbook.get(id).map(Document::output) {
                    Some(Ok(output)) => output,
                    Some(Err(e)) => {
                        self.errors
                            .push(tr_with("ST_MSG_EXPORT_FAILED", &[("error", &e)]));
                        return Task::none();
                    }
                    None => return Task::none(),
                };
                return self.start_transfer(
                    None,
                    tr_with("ST_MSG_EXPORTING", &[("name", &file_label(&path))]),
                    move |progress| {
                        veta_core::convert::export(&output, &path, &format, progress)
                            .map(|()| path)
                            .map_err(|e| error_text(&e))
                    },
                    Message::Exported,
                );
            }
            Message::ExportTo(_, None) => {}
            Message::Exported(result) => match result {
                Ok(path) => {
                    self.notice = Some(tr_with("ST_MSG_EXPORTED", &[("path", &path.display())]));
                }
                Err(e) if is_cancelled(&e) => self.notice = Some(tr("ST_MSG_EXPORT_CANCELLED")),
                Err(e) => self
                    .errors
                    .push(tr_with("ST_MSG_EXPORT_FAILED", &[("error", &e)])),
            },
            Message::TransferProgress(id, progress) => {
                if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                    t.progress = progress;
                }
            }
            Message::TransferDone(id) => self.transfers.retain(|t| t.id != id),
            Message::CancelTransfer(id) => {
                if let Some(t) = self.transfers.iter().find(|t| t.id == id) {
                    t.cancel.store(true, Ordering::Relaxed);
                }
            }
            Message::SaveRecipe => {
                if let Some(id) = self.active {
                    let name = self
                        .workbook
                        .get(id)
                        .map(|d| {
                            let title = d.title_or(&tr("ST_DOC_UNTITLED"));
                            let stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
                            format!("{stem}-recipe.json")
                        })
                        .unwrap_or_default();
                    let save = files::save(
                        tr("ST_RIBBON_BTN_SAVE_RECIPE"),
                        FileKind::Recipe,
                        name,
                        None,
                    );
                    return Task::perform(save, move |path| Message::SaveRecipeTo(id, path));
                }
            }
            Message::SaveRecipeTo(id, Some(path)) => {
                if let Some(doc) = self.workbook.get(id) {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.trim_end_matches(".json").trim_end_matches(".recipe"))
                        .unwrap_or_default()
                        .to_owned();
                    let info = veta_core::recipe::RecipeInfo {
                        name,
                        source_file: doc
                            .path()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned()),
                    };
                    let result = veta_core::recipe::to_json(doc.steps(), &info)
                        .map_err(|e| error_text(&e))
                        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
                    if let Err(e) = result {
                        self.errors
                            .push(tr_with("ST_MSG_RECIPE_SAVE_FAILED", &[("error", &e)]));
                    }
                }
            }
            Message::LoadRecipe => {
                if let Some(id) = self.active {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    let open =
                        files::open_one(tr("ST_RIBBON_BTN_LOAD_RECIPE"), vec![FileKind::Recipe]);
                    return Task::perform(open, move |path| Message::LoadRecipeFrom(id, path));
                }
            }
            Message::LoadRecipeFrom(id, Some(path)) => match std::fs::read_to_string(&path) {
                // With steps already there, ask whether to replace them.
                Ok(text) if self.workbook.get(id).is_some_and(|d| !d.steps().is_empty()) => {
                    self.dialog = Some(Dialog::ConfirmRecipe { document: id, text });
                }
                Ok(text) => self.apply_recipe(id, &text, false),
                Err(e) => self
                    .errors
                    .push(tr_with("ST_MSG_RECIPE_LOAD_FAILED", &[("error", &e)])),
            },
            Message::RecipeChoice(choice) => {
                if let Some(Dialog::ConfirmRecipe { document, text }) = self.dialog.take()
                    && let Some(replace) = choice
                {
                    self.apply_recipe(document, &text, replace);
                }
            }
            Message::SaveRecipeTo(_, None) | Message::LoadRecipeFrom(_, None) => {}
            Message::Writer(message) => {
                if let Some(Dialog::Writer { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, WriterMessage::Cancel);
                    if let Some(command) = dialog.update(message) {
                        if self.execute(id, command) {
                            self.dialog = None;
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            Message::Metadata(message) => {
                if let Some(Dialog::Metadata { document, dialog }) = &mut self.dialog {
                    let id = *document;
                    let cancel = matches!(message, MetadataMessage::Cancel);
                    if let Some(commands) = dialog.update(message) {
                        let result = match self.workbook.get_mut(id) {
                            Some(doc) => commands
                                .into_iter()
                                .try_for_each(|command| controller::execute(doc, command)),
                            None => Ok(()),
                        };
                        match result {
                            Ok(()) => self.dialog = None,
                            Err(e) => dialog.set_error(error_text(&e)),
                        }
                    } else if cancel {
                        self.dialog = None;
                    }
                }
            }
            _ => unreachable!("not a documents message"),
        }
        Task::none()
    }

    /// Ribbon actions on files.
    pub(crate) fn documents_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::Import => {
                let open = files::open_one(
                    tr("ST_RIBBON_BTN_IMPORT"),
                    vec![FileKind::Csv, FileKind::Json, FileKind::JsonLines],
                );
                return Task::perform(open, Message::ImportPicked);
            }
            Action::Export => {
                if let Some(id) = self.active {
                    match self.workbook.get(id).map(Document::output) {
                        Some(Ok(_)) => {
                            self.edit = None;
                            self.dialog = Some(Dialog::Export {
                                document: id,
                                dialog: ExportDialog::default(),
                            });
                        }
                        Some(Err(e)) => {
                            self.errors
                                .push(tr_with("ST_MSG_EXPORT_FAILED", &[("error", &e)]));
                        }
                        None => {}
                    }
                }
            }
            Action::New => self.new_document(),
            Action::LoadRecipe => return self.handle(Message::LoadRecipe),
            Action::SaveRecipe => return self.handle(Message::SaveRecipe),
            Action::Open => {
                let open = files::open_many(tr("ST_RIBBON_BTN_OPEN"), vec![FileKind::Parquet]);
                return Task::perform(open, |files| {
                    Message::FilesPicked(files.unwrap_or_default())
                });
            }
            Action::Close => {
                if let Some(id) = self.active {
                    return self.request_close(vec![id], false);
                }
            }
            Action::Save => {
                if let Some(id) = self.active {
                    let has_path = self.workbook.get(id).and_then(Document::path).is_some();
                    return if has_path {
                        self.start_save(id, None)
                    } else {
                        self.save_as(id)
                    };
                }
            }
            Action::SaveAs => {
                if let Some(id) = self.active {
                    return self.save_as(id);
                }
            }
            Action::FileMetadata => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    self.dialog = Some(Dialog::Metadata {
                        document: id,
                        dialog: MetadataDialog::new(
                            &doc.metadata().key_value,
                            column_metadata(doc),
                        ),
                    });
                }
            }
            Action::WriterSettings => {
                if let Some(id) = self.active
                    && let Some(doc) = self.workbook.get(id)
                {
                    let schema = doc.schema();
                    let columns = veta_core::io::leaf_columns(&schema).unwrap_or_else(|_| {
                        schema
                            .fields()
                            .iter()
                            .map(|f| (f.name().clone(), f.data_type().clone()))
                            .collect()
                    });
                    self.dialog = Some(Dialog::Writer {
                        document: id,
                        dialog: WriterDialog::new(doc.writer_settings().clone(), columns),
                    });
                }
            }
            _ => unreachable!("not a documents action"),
        }
        Task::none()
    }

    /// Opens a blank document named `Untitled`, `Untitled 2`, … in a new tab.
    fn new_document(&mut self) {
        let titles: Vec<String> = self
            .workbook
            .iter()
            .map(|(_, d)| d.title_or(&tr("ST_DOC_UNTITLED")))
            .collect();
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    tr("ST_DOC_UNTITLED")
                } else {
                    tr_with("ST_DOC_UNTITLED_N", &[("n", &n)])
                }
            })
            .find(|name| !titles.contains(name))
            .unwrap_or_default();
        let mut document = Document::untitled(name);
        document.set_initial_writer_settings(self.config.new_files.settings());
        self.add_tab(document);
    }

    /// Closes documents, asking about unsaved changes first. With `exit`,
    /// quits the app afterwards.
    fn request_close(&mut self, documents: Vec<DocumentId>, exit: bool) -> Task<Message> {
        self.commit_edit();
        self.menu = None;
        self.closing = Some(Closing {
            queue: documents,
            exit,
        });
        self.continue_closing()
    }

    /// Closes queued documents until one has unsaved changes (then asks) or
    /// the queue is empty.
    fn continue_closing(&mut self) -> Task<Message> {
        loop {
            let Some(closing) = &mut self.closing else {
                return Task::none();
            };
            let exit = closing.exit;
            let Some(&id) = closing.queue.first() else {
                self.closing = None;
                self.dialog = None;
                return if exit { iced::exit() } else { Task::none() };
            };
            if self.workbook.get(id).is_some_and(Document::is_modified) {
                self.active = Some(id);
                self.dialog = Some(Dialog::ConfirmClose { document: id });
                return Task::none();
            }
            closing.queue.remove(0);
            if !exit {
                self.close(id);
            }
        }
    }

    fn answer_close(&mut self, answer: CloseAnswer) -> Task<Message> {
        let Some(Dialog::ConfirmClose { document }) = self.dialog else {
            return Task::none();
        };
        match answer {
            CloseAnswer::Cancel => {
                self.dialog = None;
                self.closing = None;
                Task::none()
            }
            CloseAnswer::Discard => {
                if let Some(closing) = &mut self.closing {
                    closing.queue.retain(|&id| id != document);
                    if !closing.exit {
                        self.close(document);
                    }
                }
                self.dialog = None;
                self.continue_closing()
            }
            CloseAnswer::Save => {
                self.dialog = None;
                // Keep it at the front of the queue: when the save finishes
                // it is no longer modified and gets closed.
                self.start_save(document, None)
            }
        }
    }

    /// Asks where to save, then saves there.
    fn save_as(&mut self, id: DocumentId) -> Task<Message> {
        self.commit_edit();
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        // Imported documents are saved as Parquet.
        let name = files::suggested_name(&doc.title_or(&tr("ST_DOC_UNTITLED")), FileKind::Parquet);
        let dir = doc.path().and_then(Path::parent).map(Path::to_path_buf);
        let save = files::save(tr("ST_RIBBON_BTN_SAVE_AS"), FileKind::Parquet, name, dir);
        Task::perform(save, move |path| Message::SaveTo(id, path))
    }

    /// Saves on a worker thread; `None` saves to the document's own path.
    fn start_save(&mut self, id: DocumentId, target: Option<PathBuf>) -> Task<Message> {
        self.commit_edit();
        let Some(doc) = self.workbook.get(id) else {
            return Task::none();
        };
        match doc.save_job(target) {
            Ok(job) => {
                self.saving = Some(id);
                Task::perform(
                    blocking(move || job.run().map(Slot::new).map_err(|e| error_text(&e))),
                    move |result| Message::Saved(id, result),
                )
            }
            Err(e) => {
                self.errors
                    .push(tr_with("ST_MSG_SAVE_FAILED", &[("error", &e)]));
                Task::none()
            }
        }
    }

    /// Adds a recipe's steps (JSON `text`) after the document's, or with
    /// `replace`, in place of them (the recipe then applies to the source).
    fn apply_recipe(&mut self, id: DocumentId, text: &str, replace: bool) {
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let schema = if replace {
            doc.schema_at(0).unwrap_or_else(|| doc.schema())
        } else {
            doc.schema()
        };
        match veta_core::recipe::from_json(text, &schema) {
            Ok(steps) if steps.is_empty() => {
                self.errors.push(tr("ST_MSG_RECIPE_EMPTY"));
            }
            Ok(steps) => {
                let command = if replace {
                    Command::ReplaceSteps { steps }
                } else {
                    Command::AppendSteps { steps }
                };
                if self.execute(id, command) {
                    self.after_compute(id);
                }
            }
            Err(e) => self
                .errors
                .push(tr_with("ST_MSG_RECIPE_LOAD_FAILED", &[("error", &e)])),
        }
    }

    /// Runs `work` on a thread with progress and cancel shown in the status
    /// bar; `done` turns its result into the message to handle.
    fn start_transfer<T: Send + 'static>(
        &mut self,
        importing: Option<PathBuf>,
        label: String,
        work: impl FnOnce(&mut dyn FnMut(f32) -> bool) -> T + Send + 'static,
        done: impl Fn(T) -> Message + Send + 'static,
    ) -> Task<Message> {
        let id = self.next_transfer;
        self.next_transfer += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.transfers.push(Transfer {
            id,
            importing,
            label,
            progress: 0.0,
            cancel: cancel.clone(),
        });
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut reported = 0.0;
            let result = work(&mut |progress| {
                if progress - reported >= 0.01 {
                    reported = progress;
                    let _ = sender.unbounded_send(Message::TransferProgress(id, progress));
                }
                !cancel.load(Ordering::Relaxed)
            });
            let _ = sender.unbounded_send(done(result));
            let _ = sender.unbounded_send(Message::TransferDone(id));
        });
        Task::run(receiver, |message| message)
    }

    /// Starts opening `files` in the background. Files already open are
    /// focused instead.
    pub(crate) fn open(&mut self, files: Vec<PathBuf>) -> Task<Message> {
        let mut tasks = Vec::new();
        let options = OpenOptions::default();
        for path in files {
            if let Some(id) = self.find_open(&path) {
                self.active = Some(id);
                continue;
            }
            if self.opening.contains(&path) {
                continue;
            }
            let format = veta_core::convert::ImportFormat::from_path(&path);
            if format == Some(veta_core::convert::ImportFormat::Csv) {
                // Asks how to read it; one dialog at a time.
                if self.dialog.is_none() {
                    self.edit = None;
                    self.dialog = Some(Dialog::Import(ImportDialog::new(path)));
                }
                continue;
            }
            self.opening.push(path.clone());
            let worker_path = path.clone();
            let budget = options.memory_budget;
            if format == Some(veta_core::convert::ImportFormat::Json) {
                let label = tr_with("ST_MSG_IMPORTING", &[("name", &file_label(&path))]);
                tasks.push(self.start_transfer(
                    Some(path.clone()),
                    label,
                    move |progress| {
                        veta_core::convert::import_json(&worker_path, budget, progress)
                            .map(Loaded::new)
                            .map_err(|e| error_text(&e))
                    },
                    move |result| Message::Opened(path.clone(), result),
                ));
                continue;
            }
            tasks.push(Task::perform(
                blocking(move || {
                    Document::open(worker_path, options)
                        .map(Loaded::new)
                        .map_err(|e| error_text(&e))
                }),
                move |result| Message::Opened(path.clone(), result),
            ));
        }
        Task::batch(tasks)
    }

    /// Adds a newly opened or imported document as the active tab.
    fn add_document(&mut self, path: &Path, mut document: Document) {
        if document.path().is_none() {
            // Imported data: save with the user's defaults for new files.
            document.set_initial_writer_settings(self.config.new_files.settings());
        }
        self.add_tab(document);
        self.config.add_recent(path);
        self.save_config();
    }

    fn add_tab(&mut self, document: Document) {
        let mut grid = GridView::new(&document, self.text_size);
        // Start on the first cell, at the top left.
        grid.apply(GridEvent::Select { row: 0, column: 0 }, &document);
        // Paged files can be slow to read: scroll without waiting.
        grid.set_background(document.source_mode() == veta_core::source::SourceMode::Paged);
        let id = self.workbook.add(document);
        self.tabs.push(Tab {
            id,
            grid,
            expanded_steps: Default::default(),
        });
        self.active = Some(id);
    }

    fn find_open(&self, path: &Path) -> Option<DocumentId> {
        let target = path.canonicalize().ok()?;
        self.workbook
            .iter()
            .find(|(_, doc)| {
                doc.path().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&target)
            })
            .map(|(id, _)| id)
    }

    fn close(&mut self, id: DocumentId) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        self.tabs.remove(index);
        self.workbook.close(id);
        if self.active == Some(id) {
            // Focus the tab that took its place, or the new last one.
            self.active = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(|t| t.id);
        }
    }
}
