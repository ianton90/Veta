//! The model: open documents and their state. Changed only through the
//! [`controller`](crate::controller).

mod document;
mod metadata;
mod workbook;

pub use document::Document;
pub use metadata::{
    ColumnSettings, Compression, Encoding, FileInfo, FileMetadata, FormatVersion, KeyValue,
    RowGroupInfo, StatisticsLevel, WriterSettings,
};
pub use workbook::{DocumentId, Workbook};
