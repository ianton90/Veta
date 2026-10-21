//! Key/value metadata and writer settings commands.

use super::*;
use crate::error::invalid;

/// Plans one of this area's commands (see `super::plan`).
pub(super) fn plan(doc: &Document, command: Command) -> Result<Change> {
    match command {
        Command::SetMetadata { key, value } => {
            if key.is_empty() {
                return Err(invalid!(MetadataKeyEmpty));
            }
            let mut after = doc.metadata.clone();
            match after.key_value.iter_mut().find(|kv| kv.key == key) {
                Some(entry) => entry.value = value,
                None => after.key_value.push(KeyValue {
                    key: key.clone(),
                    value,
                }),
            }
            Ok(Change::Metadata {
                before: doc.metadata.clone(),
                after,
            })
        }
        Command::RemoveMetadata { key } => {
            let mut after = doc.metadata.clone();
            let Some(index) = after.key_value.iter().position(|kv| kv.key == key) else {
                return Err(invalid!(MetadataKeyNotFound, key = key));
            };
            after.key_value.remove(index);
            Ok(Change::Metadata {
                before: doc.metadata.clone(),
                after,
            })
        }
        Command::ReplaceColumnMetadata { column, entries } => {
            if doc.schema().field_with_name(&column).is_err() {
                return Err(invalid!(ColumnNotFound, column = column));
            }
            check_keys(&entries)?;
            let mut after = doc.metadata.clone();
            after.columns.retain(|(name, _)| *name != column);
            after.columns.push((column.clone(), entries));
            Ok(Change::Metadata {
                before: doc.metadata.clone(),
                after,
            })
        }
        Command::ReplaceMetadata(entries) => {
            check_keys(&entries)?;
            let mut after = doc.metadata.clone();
            after.key_value = entries;
            Ok(Change::Metadata {
                before: doc.metadata.clone(),
                after,
            })
        }
        Command::SetWriterSettings(settings) => {
            if settings.max_row_group_rows == 0 {
                return Err(invalid!(RowGroupSize));
            }
            Ok(Change::Writer {
                before: doc.writer.clone(),
                after: settings,
            })
        }
        other => Err(invalid!(
            WrongArea,
            area = "metadata",
            command = format!("{other:?}")
        )),
    }
}
