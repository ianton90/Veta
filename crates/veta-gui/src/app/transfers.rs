//! Importing and exporting files, on threads with progress in the status bar.

use super::documents::file_label;
use super::*;

/// The file kind an export format writes.
pub(crate) fn export_kind(choice: dialogs::ExportChoice) -> FileKind {
    match choice {
        dialogs::ExportChoice::Csv => FileKind::Csv,
        dialogs::ExportChoice::JsonLines => FileKind::JsonLines,
        dialogs::ExportChoice::JsonArray => FileKind::Json,
    }
}

impl App {
    /// Import and export messages.
    pub(crate) fn transfers_message(&mut self, message: Message) -> Task<Message> {
        match message {
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
                    Err(veta_core::Error::Cancelled) => {}
                    Err(e) => self.errors.push(tr_with(
                        "ST_MSG_IMPORT_FAILED",
                        &[("path", &path.display()), ("error", &error_text(&e))],
                    )),
                }
            }
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
                        self.errors.push(tr_with(
                            "ST_MSG_EXPORT_FAILED",
                            &[("error", &error_text(&e))],
                        ));
                        return Task::none();
                    }
                    None => return Task::none(),
                };
                return self.start_transfer(
                    None,
                    tr_with("ST_MSG_EXPORTING", &[("name", &file_label(&path))]),
                    move |progress| {
                        veta_core::convert::export(&output, &path, &format, progress).map(|()| path)
                    },
                    Message::Exported,
                );
            }
            Message::ExportTo(_, None) => {}
            Message::Exported(result) => match result {
                Ok(path) => {
                    self.notice = Some(tr_with("ST_MSG_EXPORTED", &[("path", &path.display())]));
                }
                Err(veta_core::Error::Cancelled) => {
                    self.notice = Some(tr("ST_MSG_EXPORT_CANCELLED"))
                }
                Err(e) => self.errors.push(tr_with(
                    "ST_MSG_EXPORT_FAILED",
                    &[("error", &error_text(&e))],
                )),
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
            _ => unreachable!("not a transfers message"),
        }
        Task::none()
    }

    /// Import and export ribbon actions.
    pub(crate) fn transfers_action(&mut self, action: Action) -> Task<Message> {
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
                            self.errors.push(tr_with(
                                "ST_MSG_EXPORT_FAILED",
                                &[("error", &error_text(&e))],
                            ));
                        }
                        None => {}
                    }
                }
            }
            _ => unreachable!("not a transfers action"),
        }
        Task::none()
    }

    /// Runs `work` on a thread with progress and cancel shown in the status
    /// bar; `done` turns its result into the message to handle.
    pub(crate) fn start_transfer<T: Send + 'static>(
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
}
