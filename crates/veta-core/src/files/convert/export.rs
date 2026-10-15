//! Exporting a document's output to CSV or JSON.

use super::*;

// Export.

/// Writes the pipeline's output to `target` in `format`.
pub fn export(
    output: &Pipeline,
    target: &Path,
    format: &ExportFormat,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty());
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    let temp = dir
        .unwrap_or(Path::new("."))
        .join(format!(".{name}.veta-{}.tmp", std::process::id()));
    let result = write_export(output, &temp, format, progress)
        .and_then(|()| std::fs::rename(&temp, target).map_err(Into::into));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub(super) fn write_export(
    output: &Pipeline,
    path: &Path,
    format: &ExportFormat,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<()> {
    let file = BufWriter::new(File::create(path)?);
    let rows = output.num_rows();
    let mut batches = (0..rows.max(1)).step_by(BATCH_ROWS).map(|start| {
        let end = (start + BATCH_ROWS).min(rows);
        output.read(start..end)
    });
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
        ExportFormat::Csv(options) => {
            let mut builder = arrow::csv::WriterBuilder::new()
                .with_header(options.header)
                .with_delimiter(options.delimiter);
            builder = match options.quote {
                Some(quote) => builder.with_quote(quote),
                None => builder.with_quote_style(arrow::csv::QuoteStyle::Never),
            };
            let mut writer = builder.build(file);
            for batch in batches.by_ref() {
                let batch = decimal_text(&batch?, options.decimal)?;
                writer.write(&batch)?;
                tick(batch.num_rows())?;
            }
            writer.into_inner().flush()?;
        }
        ExportFormat::JsonLines => {
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow::json::writer::LineDelimited>(file);
            for batch in batches.by_ref() {
                let batch = batch?;
                writer.write(&batch)?;
                tick(batch.num_rows())?;
            }
            writer.finish()?;
            writer.into_inner().flush()?;
        }
        ExportFormat::JsonArray => {
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, arrow::json::writer::JsonArray>(file);
            for batch in batches.by_ref() {
                let batch = batch?;
                writer.write(&batch)?;
                tick(batch.num_rows())?;
            }
            writer.finish()?;
            writer.into_inner().flush()?;
        }
    }
    Ok(())
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
                .ok_or_else(|| invalid("expected text"))?;
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
