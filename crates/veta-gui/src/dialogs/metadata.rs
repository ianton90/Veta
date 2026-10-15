//! Edit key/value metadata.

use super::*;
use iced::widget::column;

/// Edit key/value metadata: the file's (footer) and each column's (stored
/// in the schema).
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataDialog {
    /// Scope 0 is the file; the others are columns.
    scopes: Vec<MetadataScope>,
    current: usize,
    /// The Arrow schema entry, kept as is: the writer regenerates it.
    arrow_schema: Option<KeyValue>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct MetadataScope {
    /// `None` for the file.
    column: Option<String>,
    original: Vec<(String, String)>,
    entries: Vec<(String, String)>,
}

/// A choice in the scope picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChoice(usize, String);

impl std::fmt::Display for ScopeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

#[derive(Debug, Clone)]
pub enum MetadataMessage {
    Scope(ScopeChoice),
    Key(usize, String),
    Value(usize, String),
    Remove(usize),
    Add,
    Apply,
    Cancel,
}

fn pairs(entries: &[KeyValue]) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|kv| kv.key != ARROW_SCHEMA_KEY)
        .map(|kv| (kv.key.clone(), kv.value.clone().unwrap_or_default()))
        .collect()
}

impl MetadataDialog {
    /// `columns` are the top-level columns with their current metadata.
    pub fn new(file: &[KeyValue], columns: Vec<(String, Vec<KeyValue>)>) -> Self {
        let file_entries = pairs(file);
        let mut scopes = vec![MetadataScope {
            column: None,
            original: file_entries.clone(),
            entries: file_entries,
        }];
        scopes.extend(columns.into_iter().map(|(name, entries)| {
            let entries = pairs(&entries);
            MetadataScope {
                column: Some(name),
                original: entries.clone(),
                entries,
            }
        }));
        Self {
            scopes,
            current: 0,
            arrow_schema: file.iter().find(|kv| kv.key == ARROW_SCHEMA_KEY).cloned(),
            error: None,
        }
    }

    fn choices(&self) -> Vec<ScopeChoice> {
        self.scopes
            .iter()
            .enumerate()
            .map(|(i, scope)| {
                let label = match &scope.column {
                    None => "File".to_owned(),
                    Some(name) => format!("Column: {name}"),
                };
                ScopeChoice(i, label)
            })
            .collect()
    }

    fn entries_mut(&mut self) -> &mut Vec<(String, String)> {
        &mut self.scopes[self.current].entries
    }

    /// Handles a message. Returns the commands to run on apply (one per
    /// changed scope; none if nothing changed).
    pub fn update(&mut self, message: MetadataMessage) -> Option<Vec<Command>> {
        match message {
            MetadataMessage::Scope(choice) => {
                if choice.0 < self.scopes.len() {
                    self.current = choice.0;
                    self.error = None;
                }
            }
            MetadataMessage::Key(i, key) => {
                if let Some(entry) = self.entries_mut().get_mut(i) {
                    entry.0 = key;
                }
            }
            MetadataMessage::Value(i, value) => {
                if let Some(entry) = self.entries_mut().get_mut(i) {
                    entry.1 = value;
                }
            }
            MetadataMessage::Remove(i) => {
                if i < self.entries_mut().len() {
                    self.entries_mut().remove(i);
                }
            }
            MetadataMessage::Add => self.entries_mut().push((String::new(), String::new())),
            MetadataMessage::Cancel => {}
            MetadataMessage::Apply => {
                let mut commands = Vec::new();
                for scope in &self.scopes {
                    if scope.entries == scope.original {
                        continue;
                    }
                    let mut entries: Vec<KeyValue> = scope
                        .entries
                        .iter()
                        // Rows left completely empty are ignored.
                        .filter(|(k, v)| !(k.trim().is_empty() && v.is_empty()))
                        .map(|(k, v)| KeyValue {
                            key: k.trim().to_owned(),
                            value: (!v.is_empty()).then(|| v.clone()),
                        })
                        .collect();
                    match &scope.column {
                        None => {
                            if entries.iter().any(|kv| kv.key == ARROW_SCHEMA_KEY) {
                                self.error =
                                    Some(format!("{ARROW_SCHEMA_KEY} is managed automatically."));
                                return None;
                            }
                            entries.extend(self.arrow_schema.clone());
                            commands.push(Command::ReplaceMetadata(entries));
                        }
                        Some(column) => commands.push(Command::ReplaceColumnMetadata {
                            column: column.clone(),
                            entries,
                        }),
                    }
                }
                return Some(commands);
            }
        }
        None
    }

    /// Shows an error from applying the command.
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, MetadataMessage> {
        let scope = &self.scopes[self.current];
        let mut rows =
            column![row![text("Key").width(220), text("Value").width(Length::Fill)].spacing(8)]
                .spacing(6);
        for (i, (key, value)) in scope.entries.iter().enumerate() {
            rows = rows.push(
                row![
                    text_input("key", key)
                        .on_input(move |k| MetadataMessage::Key(i, k))
                        .width(220),
                    text_input("value", value)
                        .on_input(move |v| MetadataMessage::Value(i, v))
                        .width(Length::Fill),
                    button(text("Remove"))
                        .style(button::text)
                        .on_press(MetadataMessage::Remove(i)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if scope.entries.is_empty() {
            rows = rows.push(text("No metadata.").color(tokens.muted_text));
        }
        let choices = self.choices();
        let selected = choices.get(self.current).cloned();
        let mut body = column![
            text("Metadata").size(20),
            row![
                pick_list(choices, selected, MetadataMessage::Scope).width(300),
                text(match scope.column {
                    None => "Key/value pairs stored in the file's footer.",
                    Some(_) => "Key/value pairs stored with the column in the schema.",
                })
                .color(tokens.muted_text),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            scrollable(rows).height(Length::Fixed(280.0)),
            button(text("Add entry"))
                .style(button::secondary)
                .on_press(MetadataMessage::Add),
        ]
        .spacing(14);
        if self.arrow_schema.is_some() && scope.column.is_none() {
            body = body.push(
                text(format!(
                    "{ARROW_SCHEMA_KEY} (the Arrow schema) is not shown: it is rewritten on save."
                ))
                .color(tokens.muted_text),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons(
            "Apply",
            MetadataMessage::Cancel,
            MetadataMessage::Apply,
        ));
        card(body.into(), tokens, 640.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_dialog_keeps_arrow_schema_hidden() {
        let kv = |k: &str, v: &str| KeyValue {
            key: k.into(),
            value: Some(v.into()),
        };
        let mut dialog = MetadataDialog::new(
            &[kv("owner", "me"), kv(ARROW_SCHEMA_KEY, "xyz")],
            vec![("id".into(), vec![kv("unit", "m")])],
        );
        assert_eq!(dialog.scopes[0].entries.len(), 1);
        assert_eq!(
            dialog.update(MetadataMessage::Apply),
            Some(vec![]),
            "no changes"
        );
        dialog.update(MetadataMessage::Add);
        dialog.update(MetadataMessage::Key(1, " team ".into()));
        dialog.update(MetadataMessage::Add); // left empty: ignored
        dialog.update(MetadataMessage::Value(0, String::new()));
        // A column's metadata, in the same apply.
        dialog.update(MetadataMessage::Scope(ScopeChoice(1, String::new())));
        dialog.update(MetadataMessage::Value(0, "km".into()));
        let Some(commands) = dialog.update(MetadataMessage::Apply) else {
            panic!("expected commands");
        };
        let [Command::ReplaceMetadata(entries), column] = &commands[..] else {
            panic!("expected two commands: {commands:?}");
        };
        assert_eq!(
            column,
            &Command::ReplaceColumnMetadata {
                column: "id".into(),
                entries: vec![kv("unit", "km")]
            }
        );
        let entries = entries.clone();
        dialog.update(MetadataMessage::Scope(ScopeChoice(0, String::new())));
        assert_eq!(
            entries,
            vec![
                KeyValue {
                    key: "owner".into(),
                    value: None
                },
                KeyValue {
                    key: "team".into(),
                    value: None
                },
                kv(ARROW_SCHEMA_KEY, "xyz"),
            ]
        );

        dialog.update(MetadataMessage::Key(0, ARROW_SCHEMA_KEY.into()));
        assert_eq!(dialog.update(MetadataMessage::Apply), None);
        assert!(dialog.error.is_some());
    }
}
