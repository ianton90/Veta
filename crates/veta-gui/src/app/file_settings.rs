//! A file's writer settings and metadata dialogs.

use super::*;

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

impl App {
    /// File settings messages.
    pub(crate) fn file_settings_message(&mut self, message: Message) -> Task<Message> {
        match message {
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
            _ => unreachable!("not a file settings message"),
        }
        Task::none()
    }

    /// File settings ribbon actions.
    pub(crate) fn file_settings_action(&mut self, action: Action) -> Task<Message> {
        match action {
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
            _ => unreachable!("not a file settings action"),
        }
        Task::none()
    }
}
