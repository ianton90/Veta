//! JSON import: one object per line, or an array of objects.

use super::*;

// JSON.

pub(super) fn json_input<'a>(
    path: &Path,
    progress: Option<&'a mut dyn FnMut(f32) -> bool>,
) -> Result<(Box<dyn Read + 'a>, bool)> {
    let mut probe = BufReader::new(File::open(path)?);
    let array = probe
        .fill_buf()?
        .iter()
        .find(|b| !b.is_ascii_whitespace() && ![0xEF, 0xBB, 0xBF].contains(*b))
        == Some(&b'[');
    let input = Input::open(path, TextEncoding::Utf8, progress)?;
    Ok(if array {
        (Box::new(ArrayToLines::new(input)), true)
    } else {
        (Box::new(input), false)
    })
}

/// Column names and types guessed from all of a JSON file's objects (so
/// keys that first appear late aren't dropped), in the order keys first
/// appear. This reads the file once more than the import itself.
pub(super) fn json_schema(path: &Path) -> Result<SchemaRef> {
    let (input, _) = json_input(path, None)?;
    let (schema, _) = arrow::json::reader::infer_json_schema(BufReader::new(input), None)?;
    let (input, _) = json_input(path, None)?;
    let order = key_order(BufReader::new(input), usize::MAX);
    let mut fields: Vec<_> = schema.fields().iter().cloned().collect();
    fields.sort_by_key(|f| {
        order
            .iter()
            .position(|k| k == f.name())
            .unwrap_or(usize::MAX)
    });
    Ok(Arc::new(Schema::new(fields)))
}

/// Top-level keys of the first `rows` objects, in first-seen order.
pub(super) fn key_order(input: impl Read, rows: usize) -> Vec<String> {
    struct Keys<'a>(&'a mut Vec<String>);

    impl<'de> serde::de::DeserializeSeed<'de> for Keys<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            d: D,
        ) -> std::result::Result<(), D::Error> {
            d.deserialize_any(self)
        }
    }

    impl<'de> serde::de::Visitor<'de> for Keys<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<(), A::Error> {
            while let Some(key) = map.next_key::<String>()? {
                map.next_value::<serde::de::IgnoredAny>()?;
                if !self.0.contains(&key) {
                    self.0.push(key);
                }
            }
            Ok(())
        }
    }

    let mut keys = Vec::new();
    let mut stream = serde_json::Deserializer::from_reader(input);
    for _ in 0..rows {
        if serde::de::DeserializeSeed::deserialize(Keys(&mut keys), &mut stream).is_err() {
            break;
        }
    }
    keys
}

/// The first `rows` rows of a JSON file.
pub fn preview_json(path: &Path, rows: usize) -> Result<RecordBatch> {
    let schema = json_schema(path)?;
    let (input, _) = json_input(path, None)?;
    let mut reader = arrow::json::ReaderBuilder::new(schema.clone())
        .with_batch_size(rows.max(1))
        .build(BufReader::new(input))?;
    match reader.next() {
        Some(batch) => Ok(batch?),
        None => Ok(RecordBatch::new_empty(schema)),
    }
}

/// Imports a JSON file (one object per line, or an array of objects) as a
/// new, unsaved document. Types are guessed from all objects.
pub fn import_json(
    path: &Path,
    memory_budget: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Document> {
    let source = read_json_data(path, memory_budget, progress)?;
    Ok(Document::imported(file_name(path), source))
}

/// Reads a JSON file into memory, or into a paged temporary file when it
/// comes to more than `memory_budget` bytes.
pub(crate) fn read_json_data(
    path: &Path,
    memory_budget: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Arc<dyn DataSource>> {
    let schema = json_schema(path)?;
    let mut cancelled_flag = false;
    let source = {
        let mut progress = |f: f32| {
            let go = progress(f);
            if !go {
                cancelled_flag = true;
            }
            go
        };
        let (input, _) = json_input(path, Some(&mut progress))?;
        let reader = arrow::json::ReaderBuilder::new(schema.clone())
            .with_batch_size(BATCH_ROWS)
            .build(BufReader::new(input))?;
        store(
            &schema,
            reader.map(|b| b.map_err(Error::from)),
            memory_budget,
        )
    };
    cancelled(source, cancelled_flag)
}
