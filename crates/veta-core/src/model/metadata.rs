/// File-level metadata stored in the Parquet footer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMetadata {
    /// Key/value pairs, in file order. Parquet allows a key without a value.
    pub key_value: Vec<KeyValue>,
    /// The `created_by` string of the writer that produced the file.
    pub created_by: Option<String>,
}

impl FileMetadata {
    pub fn get(&self, key: &str) -> Option<&KeyValue> {
        self.key_value.iter().find(|kv| kv.key == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: Option<String>,
}

/// Settings used when writing the document back to Parquet.
///
/// Read from the source file on open (#15) and edited through the writer
/// settings dialog (#27).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriterSettings {}
