//! Exporting a document's output to CSV or JSON.

use super::*;
use crate::error::invalid;
use std::sync::Mutex;

// Export.

/// Writes the pipeline's output to `target` in `format`.
pub fn export(
    output: &Pipeline,
    target: &Path,
    format: &ExportFormat,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let (replacement, file) = crate::files::replace::Replacement::create(target)?;
    write_export(output, file, format, progress)?;
    replacement.commit()
}

pub(super) fn write_export(
    output: &Pipeline,
    file: File,
    format: &ExportFormat,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let mut file = BufWriter::new(file);
    let rows = output.num_rows();
    let mut done = 0;
    let mut tick = |n: usize| {
        done += n;
        if progress(done as f32 / rows.max(1) as f32) {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    };
    match format {
        ExportFormat::Csv(_) | ExportFormat::JsonLines => {
            // Pieces are read and turned into text on several threads, then
            // written in order. Written texts' buffers go back to the threads:
            // a fresh buffer for every piece would have the system map and
            // clear new memory each time.
            let piece_format = format.clone();
            let spare: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
            let buffers = spare.clone();
            let pieces =
                output.read_parallel(output.evaluated(), 0..rows, None, move |batches, first| {
                    let n: usize = batches.iter().map(RecordBatch::num_rows).sum();
                    let out = buffers.lock().ok().and_then(|mut b| b.pop());
                    let text = crate::profile::time("export: turning rows into text", || {
                        format_piece(&piece_format, &batches, first == 0, out)
                    })?;
                    Ok((text, n))
                });
            let mut pieces = pieces.into_iter();
            let mut written = false;
            while let Some(piece) =
                crate::profile::time("export: waiting for text", || pieces.next())
            {
                let (text, n) = piece?;
                crate::profile::time("export: writing", || file.write_all(&text))?;
                if let Ok(mut spare) = spare.lock() {
                    spare.push(text);
                }
                written = true;
                tick(n)?;
            }
            // An empty table still gets its header line.
            if !written && let ExportFormat::Csv(_) = format {
                let empty = RecordBatch::new_empty(output.schema());
                file.write_all(&format_piece(format, &[empty], true, None)?)?;
            }
            file.flush()?;
        }
        ExportFormat::JsonArray => {
            let pieces =
                output.read_parallel(output.evaluated(), 0..rows, None, |batches, _| Ok(batches));
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow::json::writer::JsonArray>(file);
            for piece in pieces {
                for batch in piece? {
                    writer.write(&batch)?;
                    tick(batch.num_rows())?;
                }
            }
            writer.finish()?;
            writer.into_inner().flush()?;
        }
    }
    Ok(())
}

/// `batches` as CSV (with the header line when `first`) or JSON lines,
/// written into `out` (emptied first) if given.
fn format_piece(
    format: &ExportFormat,
    batches: &[RecordBatch],
    first: bool,
    out: Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut out = out.unwrap_or_default();
    out.clear();
    match format {
        ExportFormat::Csv(options) => {
            let builder = arrow::csv::WriterBuilder::new()
                .with_header(options.header && first)
                .with_delimiter(options.delimiter);
            let builder = match options.quote {
                Some(quote) => builder.with_quote(quote),
                None => builder.with_quote_style(arrow::csv::QuoteStyle::Never),
            };
            let mut writer = builder.build(&mut out);
            for batch in batches {
                writer.write(&decimal_text(batch, options.decimal)?)?;
            }
        }
        ExportFormat::JsonLines => {
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow::json::writer::LineDelimited>(&mut out);
            for batch in batches {
                writer.write(batch)?;
            }
            writer.finish()?;
        }
        ExportFormat::JsonArray => return Err(invalid!(JsonArrayWhole)),
    }
    Ok(out)
}

/// With a decimal separator other than `.`, writes numbers as text using it.
pub(super) fn decimal_text(batch: &RecordBatch, decimal: char) -> Result<RecordBatch> {
    if decimal == '.' {
        return Ok(batch.clone());
    }
    let mut fields = Vec::new();
    let mut columns = Vec::new();
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        let t = field.data_type();
        let fractional = t.is_floating()
            || matches!(
                t,
                DataType::Decimal32(..)
                    | DataType::Decimal64(..)
                    | DataType::Decimal128(..)
                    | DataType::Decimal256(..)
            );
        if fractional {
            let text = arrow::compute::cast(column, &DataType::Utf8)?;
            let text = text
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| invalid!(ExpectedText))?;
            let replaced: StringArray = text
                .iter()
                .map(|v| v.map(|v| v.replace('.', &decimal.to_string())))
                .collect();
            fields.push(Field::new(
                field.name(),
                DataType::Utf8,
                field.is_nullable(),
            ));
            columns.push(Arc::new(replaced) as ArrayRef);
        } else {
            fields.push(field.as_ref().clone());
            columns.push(column.clone());
        }
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}
