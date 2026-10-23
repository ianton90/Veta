//! CSV import: reading text columns and converting them to their types.

use super::*;
use crate::error::invalid;

// CSV.

pub(super) fn csv_format(options: &CsvOptions) -> arrow::csv::reader::Format {
    arrow::csv::reader::Format::default()
        .with_header(options.header)
        .with_delimiter(options.delimiter)
        // No quoting: a byte that doesn't occur in text files.
        .with_quote(options.quote.unwrap_or(0))
        .with_truncated_rows(true)
}

/// Converts text columns to the types of `schema`. Returns the converted
/// columns, the rows whose value didn't fit, per column, and the original
/// text of those values.
pub(super) fn convert_columns(
    batch: &RecordBatch,
    schema: &SchemaRef,
    options: &CsvOptions,
) -> Result<Vec<(ArrayRef, Vec<usize>)>> {
    // Columns convert independently: all at the same time.
    std::thread::scope(|scope| {
        let work: Vec<_> = batch
            .columns()
            .iter()
            .zip(schema.fields())
            .map(|(column, field)| scope.spawn(move || convert_column(column, field, options)))
            .collect();
        work.into_iter()
            .map(|w| w.join().unwrap_or_else(|_| Err(invalid!(ImportFailed))))
            .collect()
    })
}

/// One CSV column read as text, converted to `field`'s type, with the rows
/// whose text didn't convert.
fn convert_column(
    column: &ArrayRef,
    field: &arrow::datatypes::FieldRef,
    options: &CsvOptions,
) -> Result<(ArrayRef, Vec<usize>)> {
    if field.data_type() == &DataType::Utf8 {
        return Ok((column.clone(), Vec::new()));
    }
    let converted = Cast {
        column: field.name().clone(),
        to: field.data_type().clone(),
        options: CastOptions {
            decimal: options.decimal,
            thousands: None,
            format: options.date_format.clone().filter(|_| {
                matches!(
                    field.data_type(),
                    DataType::Date32 | DataType::Timestamp(..)
                )
            }),
            on_error: OnError::Null,
        },
    }
    .convert(column)?;
    let failed = (0..column.len())
        .filter(|&i| column.is_valid(i) && converted.is_null(i))
        .collect();
    Ok((converted, failed))
}

/// Name of the column holding a column's invalid values.
pub fn error_column_name(column: &str) -> String {
    format!("{column} errors")
}

/// The imported layout: typed columns, each followed (with error columns)
/// by a text column of the values that didn't fit. Returns the schema and,
/// per output column, the input column it comes from and whether it is an
/// error column.
pub(super) fn import_layout(
    schema: &SchemaRef,
    on_error: OnError,
) -> (SchemaRef, Vec<(usize, bool)>) {
    let mut fields = Vec::new();
    let mut layout = Vec::new();
    for (i, field) in schema.fields().iter().enumerate() {
        fields.push(field.as_ref().clone());
        layout.push((i, false));
        if on_error == OnError::ErrorColumn && field.data_type() != &DataType::Utf8 {
            let mut name = error_column_name(field.name());
            while schema.fields().iter().any(|f| f.name() == &name) {
                name.push('_');
            }
            fields.push(Field::new(name, DataType::Utf8, true));
            layout.push((i, true));
        }
    }
    (Arc::new(Schema::new(fields)), layout)
}

/// Converts one batch of text into the imported layout, counting failures
/// per input column; with [`OnError::Fail`], the first failure is an error.
pub(super) fn import_batch(
    text: &RecordBatch,
    schema: &SchemaRef,
    layout: &(SchemaRef, Vec<(usize, bool)>),
    options: &CsvOptions,
    first_row: usize,
    failures: &mut [u64],
) -> Result<RecordBatch> {
    let converted = convert_columns(text, schema, options)?;
    for (i, (_, failed)) in converted.iter().enumerate() {
        failures[i] += failed.len() as u64;
        if options.on_error == OnError::Fail
            && let Some(&row) = failed.first()
        {
            let value = array_text(text.column(i), row);
            return Err(invalid!(
                RowValueType,
                row = first_row + row + 1,
                value = value,
                column = schema.field(i).name(),
                data_type = crate::display::type_name(schema.field(i).data_type())
            ));
        }
    }
    let columns = layout
        .1
        .iter()
        .map(|&(i, error)| {
            if !error {
                return converted[i].0.clone();
            }
            let failed = &converted[i].1;
            let mut values: Vec<Option<String>> = vec![None; text.num_rows()];
            for &row in failed {
                values[row] = Some(array_text(text.column(i), row));
            }
            Arc::new(StringArray::from(values)) as ArrayRef
        })
        .collect();
    Ok(RecordBatch::try_new(layout.0.clone(), columns)?)
}

pub(super) fn array_text(array: &ArrayRef, row: usize) -> String {
    array
        .as_any()
        .downcast_ref::<StringArray>()
        .filter(|a| a.is_valid(row))
        .map(|a| a.value(row).to_owned())
        .unwrap_or_default()
}

pub(super) fn text_schema(schema: &Schema) -> SchemaRef {
    Arc::new(Schema::new(
        schema
            .fields()
            .iter()
            .map(|f| Field::new(f.name(), DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ))
}

/// Most text read to guess types or preview rows: with a quote that is never
/// closed, the first record would run to the end of the file (#160).
const SAMPLE_BYTES: u64 = 256 * 1024 * 1024;

/// Column names and guessed types.
pub(super) fn infer_csv(path: &Path, options: &CsvOptions) -> Result<SchemaRef> {
    let input = Input::open(path, options.encoding, None)?.take(SAMPLE_BYTES);
    let (schema, _) = csv_format(options).infer_schema(input, Some(INFER_ROWS))?;
    let mut fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|f| Field::new(f.name(), f.data_type().clone(), true))
        .collect();
    // With a decimal comma, numbers look like text to arrow: check the
    // sample ourselves.
    if options.decimal != '.' && fields.iter().any(|f| f.data_type() == &DataType::Utf8) {
        let sample = read_csv_text(path, options, &text_schema(&schema), INFER_ROWS)?;
        for (i, field) in fields.iter_mut().enumerate() {
            if field.data_type() != &DataType::Utf8 {
                continue;
            }
            let column = sample.column(i);
            let cast = Cast {
                column: field.name().clone(),
                to: DataType::Float64,
                options: CastOptions {
                    decimal: options.decimal,
                    ..CastOptions::default()
                },
            };
            let preview = cast.preview(column);
            if column.len() > column.null_count() && preview.failed == 0 {
                *field = Field::new(field.name(), DataType::Float64, true);
            }
        }
    }
    Ok(Arc::new(Schema::new(fields)))
}

/// The first `rows` rows of the file, all as text.
pub(super) fn read_csv_text(
    path: &Path,
    options: &CsvOptions,
    schema: &SchemaRef,
    rows: usize,
) -> Result<RecordBatch> {
    let input = Input::open(path, options.encoding, None)?.take(SAMPLE_BYTES);
    let mut reader = arrow::csv::ReaderBuilder::new(schema.clone())
        .with_format(csv_format(options))
        .with_batch_size(rows.max(1))
        .build(input)?;
    match reader.next() {
        Some(batch) => Ok(batch?),
        None => Ok(RecordBatch::new_empty(schema.clone())),
    }
}

/// The schema a CSV file will be imported with.
pub(super) fn csv_schema(path: &Path, options: &CsvOptions) -> Result<SchemaRef> {
    let inferred = infer_csv(path, options)?;
    match &options.types {
        None => Ok(inferred),
        Some(types) => {
            if types.len() != inferred.fields().len() {
                return Err(invalid!(
                    ColumnCount,
                    found = inferred.fields().len(),
                    expected = types.len()
                ));
            }
            Ok(Arc::new(Schema::new(
                inferred
                    .fields()
                    .iter()
                    .zip(types)
                    .map(|(f, t)| Field::new(f.name(), t.clone(), true))
                    .collect::<Vec<_>>(),
            )))
        }
    }
}

/// The first `rows` rows of a CSV file as they would be imported.
pub fn preview_csv(path: &Path, options: &CsvOptions, rows: usize) -> Result<CsvPreview> {
    let schema = csv_schema(path, options)?;
    let text = read_csv_text(path, options, &text_schema(&schema), rows)?;
    let converted = convert_columns(&text, &schema, options)?;
    let failures = converted.iter().map(|(_, f)| f.len()).collect();
    let columns = converted.into_iter().map(|(c, _)| c).collect();
    let rows = RecordBatch::try_new(schema.clone(), columns)?;
    Ok(CsvPreview {
        schema,
        rows,
        failures,
    })
}

/// Imports a CSV file as a new, unsaved document, with a report of the
/// values that didn't fit their column's type. Results larger than
/// `memory_budget` bytes stay on disk and are paged.
pub fn import_csv(
    path: &Path,
    options: &CsvOptions,
    memory_budget: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<(Document, ImportReport)> {
    let schema = csv_schema(path, options)?;
    let layout = import_layout(&schema, options.on_error);
    let mut failures = vec![0u64; schema.fields().len()];
    let mut rows = 0usize;
    let mut input = Input::open(path, options.encoding, Some(progress))?;
    let source = {
        // Parsed on all cores, converted in order.
        let reader = super::chunks::ParallelCsv::new(
            &mut input,
            text_schema(&schema),
            csv_format(options),
            options.header,
            options.quote,
            BATCH_ROWS,
        );
        let batches = reader.map(|text| {
            let text = text?;
            let batch = import_batch(&text, &schema, &layout, options, rows, &mut failures)?;
            rows += text.num_rows();
            Ok(batch)
        });
        store(&layout.0, batches, memory_budget)
    };
    let source = cancelled(source, input.cancelled)?;
    // Error columns of columns where everything fitted are left out.
    let keep: Vec<usize> = layout
        .1
        .iter()
        .enumerate()
        .filter(|(_, (i, error))| !error || failures[*i] > 0)
        .map(|(k, _)| k)
        .collect();
    let source = if keep.len() == layout.1.len() {
        source
    } else {
        Arc::new(Projected::new(source, keep)?)
    };
    let report = ImportReport {
        rows: rows as u64,
        failures: schema
            .fields()
            .iter()
            .zip(&failures)
            .filter(|(_, n)| **n > 0)
            .map(|(f, n)| (f.name().clone(), *n))
            .collect(),
        on_error: Some(options.on_error),
    };
    Ok((Document::imported(file_name(path), source), report))
}

/// Converts a CSV file straight into a new Parquet file at `target`, as
/// importing and then saving it would, without the temporary copy in
/// between. Returns `None`, leaving `target` alone, when values don't fit
/// their column's type and go to error columns: which error columns to keep
/// is only known at the end, so import the file instead.
pub fn convert_csv(
    path: &Path,
    options: &CsvOptions,
    target: &Path,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Option<ImportReport>> {
    let schema = csv_schema(path, options)?;
    // No error columns: with them, the first failure stops the conversion.
    let layout = import_layout(&schema, OnError::Null);
    let mut failures = vec![0u64; schema.fields().len()];
    let mut rows = 0usize;
    let mut needs_errors = false;
    let mut input = Input::open(path, options.encoding, Some(progress))?;
    let written = {
        let reader = super::chunks::ParallelCsv::new(
            &mut input,
            text_schema(&schema),
            csv_format(options),
            options.header,
            options.quote,
            BATCH_ROWS,
        );
        let batches = reader.map(|text| {
            let text = text?;
            let batch = import_batch(&text, &schema, &layout, options, rows, &mut failures)?;
            if options.on_error == OnError::ErrorColumn && failures.iter().any(|&n| n > 0) {
                needs_errors = true;
                return Err(invalid!(ErrorColumnsNeeded));
            }
            rows += text.num_rows();
            Ok(batch)
        });
        crate::files::parquet::write_new_file(&layout.0, batches, target)
    };
    if needs_errors {
        return Ok(None);
    }
    cancelled(written, input.cancelled)?;
    Ok(Some(ImportReport {
        rows: rows as u64,
        failures: schema
            .fields()
            .iter()
            .zip(&failures)
            .filter(|(_, n)| **n > 0)
            .map(|(f, n)| (f.name().clone(), *n))
            .collect(),
        on_error: Some(options.on_error),
    }))
}

/// Some of a source's columns.
#[derive(Debug)]
pub(super) struct Projected {
    inner: Arc<dyn DataSource>,
    schema: SchemaRef,
    columns: Vec<usize>,
}

impl Projected {
    fn new(inner: Arc<dyn DataSource>, columns: Vec<usize>) -> Result<Self> {
        let schema = Arc::new(inner.schema().project(&columns)?);
        Ok(Self {
            inner,
            schema,
            columns,
        })
    }
}

impl DataSource for Projected {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn num_rows(&self) -> usize {
        self.inner.num_rows()
    }

    fn mode(&self) -> crate::source::SourceMode {
        self.inner.mode()
    }

    fn read(&self, range: std::ops::Range<usize>) -> Result<RecordBatch> {
        Ok(self.inner.read(range)?.project(&self.columns)?)
    }
}
