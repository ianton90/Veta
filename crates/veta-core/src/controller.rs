//! The controller: the only code that mutates a [`Document`].
//!
//! Undo/redo history is added in #21.

use crate::command::Command;
use crate::error::{Error, Result};
use crate::model::{Document, KeyValue};

/// Validates `command` and applies it to `document`.
///
/// On error the document is left unchanged.
pub fn execute(document: &mut Document, command: Command) -> Result<()> {
    match command {
        Command::SetMetadata { key, value } => {
            if key.is_empty() {
                return Err(Error::InvalidCommand("metadata key cannot be empty".into()));
            }
            let entries = &mut document.metadata.key_value;
            match entries.iter_mut().find(|kv| kv.key == key) {
                Some(entry) => entry.value = value,
                None => entries.push(KeyValue { key, value }),
            }
        }
        Command::RemoveMetadata { key } => {
            let entries = &mut document.metadata.key_value;
            let Some(index) = entries.iter().position(|kv| kv.key == key) else {
                return Err(Error::InvalidCommand(format!("no metadata key {key:?}")));
            };
            entries.remove(index);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(key: &str, value: &str) -> Command {
        Command::SetMetadata {
            key: key.into(),
            value: Some(value.into()),
        }
    }

    #[test]
    fn set_metadata_adds_then_replaces() {
        let mut doc = Document::new();
        execute(&mut doc, set("owner", "a")).unwrap();
        execute(&mut doc, set("owner", "b")).unwrap();

        let entries = &doc.metadata().key_value;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value.as_deref(), Some("b"));
    }

    #[test]
    fn empty_key_is_rejected() {
        let mut doc = Document::new();
        let err = execute(&mut doc, set("", "x")).unwrap_err();
        assert!(matches!(err, Error::InvalidCommand(_)));
        assert!(doc.metadata().key_value.is_empty());
    }

    #[test]
    fn remove_metadata() {
        let mut doc = Document::new();
        execute(&mut doc, set("owner", "a")).unwrap();
        execute(
            &mut doc,
            Command::RemoveMetadata {
                key: "owner".into(),
            },
        )
        .unwrap();
        assert!(doc.metadata().get("owner").is_none());

        let err = execute(
            &mut doc,
            Command::RemoveMetadata {
                key: "owner".into(),
            },
        );
        assert!(matches!(err, Err(Error::InvalidCommand(_))));
    }
}
