//! Error type shared by all of `veta-core`.

use std::fmt;

use arrow::error::ArrowError;
use parquet::errors::ParquetError;

use crate::model::DocumentId;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parquet(ParquetError),
    Arrow(ArrowError),
    /// A command was rejected by the controller before changing anything.
    InvalidCommand(String),
    DocumentNotFound(DocumentId),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Parquet(e) => write!(f, "Parquet error: {e}"),
            Error::Arrow(e) => write!(f, "Arrow error: {e}"),
            Error::InvalidCommand(msg) => {
                // Messages are written lowercase to compose; show them as a
                // sentence on their own.
                let mut chars = msg.chars();
                match chars.next() {
                    Some(first) => write!(f, "{}{}", first.to_uppercase(), chars.as_str()),
                    None => Ok(()),
                }
            }
            Error::DocumentNotFound(id) => write!(f, "document {id} not found"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Parquet(e) => Some(e),
            Error::Arrow(e) => Some(e),
            Error::InvalidCommand(_) | Error::DocumentNotFound(_) => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<ParquetError> for Error {
    fn from(e: ParquetError) -> Self {
        Error::Parquet(e)
    }
}

impl From<ArrowError> for Error {
    fn from(e: ArrowError) -> Self {
        Error::Arrow(e)
    }
}
