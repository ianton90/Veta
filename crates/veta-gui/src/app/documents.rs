//! Opening, saving and closing files, and the tabs.

use super::*;

pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
    /// Messages about opening, saving and closing files, and the tabs.
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
                    Err(veta_core::Error::Cancelled) => {}
                    Err(e) => self.errors.push(tr_with(
                        "ST_MSG_OPEN_FAILED",
                        &[("path", &path.display()), ("error", &error_text(&e))],
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
                            .push(tr_with("ST_MSG_SAVE_FAILED", &[("error", &error_text(&e))]));
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
            _ => unreachable!("not a documents message"),
        }
        Task::none()
    }

    pub(crate) fn documents_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::New => self.new_document(),
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
                Task::perform(blocking(move || job.run().map(Slot::new)), move |result| {
                    Message::Saved(id, result)
                })
            }
            Err(e) => {
                self.errors
                    .push(tr_with("ST_MSG_SAVE_FAILED", &[("error", &error_text(&e))]));
                Task::none()
            }
        }
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
                    },
                    move |result| Message::Opened(path.clone(), result),
                ));
                continue;
            }
            tasks.push(Task::perform(
                blocking(move || Document::open(worker_path, options).map(Loaded::new)),
                move |result| Message::Opened(path.clone(), result),
            ));
        }
        Task::batch(tasks)
    }

    /// Adds a newly opened or imported document as the active tab.
    pub(crate) fn add_document(&mut self, path: &Path, mut document: Document) {
        if document.path().is_none() {
            // Imported data: save with the user's defaults for new files.
            document.set_initial_writer_settings(self.config.new_files.settings());
        }
        self.add_tab(document);
        self.config.add_recent(path);
        self.save_config();
    }

    fn add_tab(&mut self, document: Document) {
        let mut grid = GridView::new(&document, self.table_text_size());
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
