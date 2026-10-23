//! Errors from `veta-core` in the app's language: each error code is the
//! tag `ERR_<CODE>` (e.g. `ERR_COLUMN_NOT_FOUND`), with the error's values
//! for its placeholders (decision 56).

use veta_core::Error;
use veta_core::error::ErrorCode;

use super::{Translations, tr, tr_with};

/// `error` as a sentence in the app's language.
pub fn error_text(error: &Error) -> String {
    let text = message(error);
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// The tag of an error code: `ColumnNotFound` → `ERR_COLUMN_NOT_FOUND`.
pub fn error_tag(code: ErrorCode) -> String {
    let mut tag = String::from("ERR_");
    for (i, c) in code.name().chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            tag.push('_');
        }
        tag.push(c.to_ascii_uppercase());
    }
    tag
}

/// The message, not capitalised (to compose with others).
fn message(error: &Error) -> String {
    match error {
        Error::Io(e) => tr_with("ERR_IO", &[("error", e)]),
        Error::Parquet(e) => tr_with("ERR_PARQUET", &[("error", e)]),
        Error::Arrow(e) => tr_with("ERR_ARROW", &[("error", e)]),
        Error::DocumentNotFound(id) => tr_with("ERR_DOCUMENT_NOT_FOUND", &[("id", id)]),
        Error::Cancelled => tr("ERR_CANCELLED"),
        Error::Invalid(invalid) => {
            let tag = error_tag(invalid.code);
            // A code without a text yet reads in the core's English. The
            // lock is released before `tr` takes it again: a second read
            // while a language change waits would deadlock.
            let has = Translations::global().has(&tag);
            let mut text = match has {
                true => tr(&tag),
                false => invalid.code.english().to_owned(),
            };
            for (name, value) in &invalid.args {
                text = text.replace(&format!("{{{name}}}"), value);
            }
            if let Some(cause) = &invalid.cause {
                text = text.replace("{cause}", &message(cause));
            }
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_english() {
        assert_eq!(error_tag(ErrorCode::ColumnNotFound), "ERR_COLUMN_NOT_FOUND");
        let error = Error::Invalid(veta_core::error::Invalid {
            code: ErrorCode::RowColumn,
            args: vec![("row", "3".into()), ("column", "id".into())],
            cause: Some(std::sync::Arc::new(Error::Invalid(
                veta_core::error::Invalid {
                    code: ErrorCode::ExpectedNumber,
                    args: Vec::new(),
                    cause: None,
                },
            ))),
        });
        // Same words as the core's English while the app is in English.
        assert_eq!(error_text(&error), error.to_string());
    }
}
