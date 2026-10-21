//! Error type shared by all of `veta-core`. Rejected commands and inputs are
//! codes with their values ([`Invalid`]); `Display` gives plain English, and
//! front ends word them in their own language from the code (decision 56).

use std::fmt;
use std::sync::Arc;

use arrow::error::ArrowError;
use parquet::errors::ParquetError;

use crate::model::DocumentId;

mod codes;

pub use codes::ErrorCode;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Cloneable: library errors are shared, so a broken step can keep its error
/// and still return it.
#[derive(Debug, Clone)]
pub enum Error {
    Io(Arc<std::io::Error>),
    Parquet(Arc<ParquetError>),
    Arrow(Arc<ArrowError>),
    /// A command or input was rejected: what went wrong, as a code.
    Invalid(Invalid),
    DocumentNotFound(DocumentId),
    /// A long-running computation was cancelled.
    Cancelled,
}

/// A rejected command or input: its code, the values its message names
/// (`{column}`, `{row}`, …) and the error it wraps, if any (`{cause}`).
#[derive(Debug, Clone)]
pub struct Invalid {
    pub code: ErrorCode,
    pub args: Vec<(&'static str, String)>,
    pub cause: Option<Arc<Error>>,
}

impl Invalid {
    /// The value of the placeholder `name`.
    pub fn arg(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// An [`Error::Invalid`]: `invalid!(ColumnNotFound, column = name)`, or with
/// the error it wraps: `invalid!(RowValue, row = 3; cause = e)`.
macro_rules! invalid {
    ($code:ident $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::error::Error::Invalid($crate::error::Invalid {
            code: $crate::error::ErrorCode::$code,
            args: vec![$((stringify!($name), $value.to_string())),*],
            cause: None,
        })
    };
    ($code:ident $(, $name:ident = $value:expr)* ; cause = $cause:expr) => {
        $crate::error::Error::Invalid($crate::error::Invalid {
            code: $crate::error::ErrorCode::$code,
            args: vec![$((stringify!($name), $value.to_string())),*],
            cause: Some(::std::sync::Arc::<$crate::error::Error>::from($cause)),
        })
    };
}
pub(crate) use invalid;

impl Error {
    /// The message in plain English, not capitalised (to compose).
    fn message(&self) -> String {
        match self {
            Error::Io(e) => format!("I/O error: {e}"),
            Error::Parquet(e) => format!("Parquet error: {e}"),
            Error::Arrow(e) => format!("Arrow error: {e}"),
            Error::Invalid(invalid) => {
                let mut text = invalid.code.english().to_owned();
                for (name, value) in &invalid.args {
                    text = text.replace(&format!("{{{name}}}"), value);
                }
                if let Some(cause) = &invalid.cause {
                    text = text.replace("{cause}", &cause.message());
                }
                text
            }
            Error::DocumentNotFound(id) => format!("document {id} not found"),
            Error::Cancelled => "cancelled".into(),
        }
    }

    /// The code of an [`Error::Invalid`].
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Error::Invalid(invalid) => Some(invalid.code),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Messages are written lowercase to compose; show them as a
        // sentence on their own.
        let message = self.message();
        let mut chars = message.chars();
        match chars.next() {
            Some(first) => write!(f, "{}{}", first.to_uppercase(), chars.as_str()),
            None => Ok(()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e.as_ref()),
            Error::Parquet(e) => Some(e.as_ref()),
            Error::Arrow(e) => Some(e.as_ref()),
            Error::Invalid(invalid) => invalid
                .cause
                .as_deref()
                .map(|e| e as &(dyn std::error::Error + 'static)),
            Error::DocumentNotFound(_) | Error::Cancelled => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(Arc::new(e))
    }
}

impl From<ParquetError> for Error {
    fn from(e: ParquetError) -> Self {
        Error::Parquet(Arc::new(e))
    }
}

impl From<ArrowError> for Error {
    fn from(e: ArrowError) -> Self {
        Error::Arrow(Arc::new(e))
    }
}
