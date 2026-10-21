//! Changing a column's type, with locale options for reading text.

use crate::error::invalid;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Date32Array, StringArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
};
use arrow::compute::{CastOptions as ArrowCastOptions, can_cast_types, cast_with_options};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;

use crate::display::type_name;
use crate::error::Result;
use crate::values::is_text;

/// Converts `column` to type `to`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cast {
    pub column: String,
    pub to: DataType,
    pub options: CastOptions,
}

/// How text is read when converting it.
#[derive(Debug, Clone, PartialEq)]
pub struct CastOptions {
    /// Decimal separator in numbers.
    pub decimal: char,
    /// Thousands separator in numbers, removed before parsing.
    pub thousands: Option<char>,
    /// Date/time format (`%d/%m/%Y`, …); `None` reads ISO 8601.
    pub format: Option<String>,
    pub on_error: OnError,
}

impl Default for CastOptions {
    fn default() -> Self {
        Self {
            decimal: '.',
            thousands: None,
            format: None,
            on_error: OnError::Null,
        }
    }
}

/// What happens to values that can't be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    /// They become null.
    Null,
    /// Reading (and saving) fails.
    Fail,
    /// They become null, and the original text goes to an extra column.
    ErrorColumn,
}

impl OnError {
    pub const ALL: [OnError; 3] = [Self::Null, Self::Fail, Self::ErrorColumn];
}

/// Values that failed to convert in a sample.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CastPreview {
    pub checked: usize,
    pub failed: usize,
    /// A few of the failed values, as text.
    pub examples: Vec<String>,
}

impl Cast {
    /// Name of the extra column holding values that failed to convert.
    pub fn error_column(&self) -> String {
        format!("{} errors", self.column)
    }

    /// Checks the cast against the input schema and returns the output
    /// fields.
    pub(crate) fn output_fields(&self, schema: &Schema) -> Result<Vec<Field>> {
        let index = schema
            .index_of(&self.column)
            .map_err(|_| invalid!(ColumnNotFound, column = self.column))?;
        let from = schema.field(index).data_type();
        let readable = if is_text(from) {
            // Nested values are read from JSON text.
            self.to.is_nested() || can_cast_types(&DataType::Utf8, &self.to)
        } else {
            can_cast_types(from, &self.to)
        };
        if !readable {
            return Err(invalid!(
                CastImpossible,
                column = self.column,
                from = type_name(from),
                to = type_name(&self.to)
            ));
        }
        if let Some(format) = &self.options.format {
            if !matches!(self.to, DataType::Date32 | DataType::Timestamp(..)) {
                return Err(invalid!(FormatOnlyDates));
            }
            if !is_text(from) {
                return Err(invalid!(FormatOnlyText));
            }
            check_format(format)?;
        }
        if self.options.thousands == Some(self.options.decimal) {
            return Err(invalid!(SeparatorsEqual));
        }
        let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
        fields[index] = Field::new(&self.column, self.to.clone(), true);
        if self.options.on_error == OnError::ErrorColumn {
            let name = self.error_column();
            if schema.fields().iter().any(|f| f.name() == &name) {
                return Err(invalid!(ColumnExists, column = name));
            }
            fields.insert(index + 1, Field::new(name, DataType::Utf8, true));
        }
        Ok(fields)
    }

    /// Converts the column in `batch`; `schema` is the output schema.
    pub(crate) fn apply(&self, batch: &RecordBatch, schema: &Arc<Schema>) -> Result<RecordBatch> {
        let index = batch
            .schema()
            .index_of(&self.column)
            .map_err(|_| invalid!(ColumnNotFound, column = self.column))?;
        let input = batch.column(index);
        let output = self.convert(input)?;
        let failed = failures(input, &output);
        let mut columns = batch.columns().to_vec();
        match self.options.on_error {
            OnError::Fail => {
                if let Some(row) = failed.first() {
                    return Err(invalid!(
                        CastValue,
                        value = text_at(input, *row),
                        column = self.column,
                        data_type = type_name(&self.to)
                    ));
                }
            }
            OnError::Null => {}
            OnError::ErrorColumn => {
                let mut errors: Vec<Option<String>> = vec![None; input.len()];
                for &row in &failed {
                    errors[row] = Some(text_at(input, row));
                }
                columns.insert(index + 1, Arc::new(StringArray::from(errors)));
            }
        }
        columns[index] = output;
        Ok(RecordBatch::try_new(schema.clone(), columns)?)
    }

    /// The first row of `batch` whose value can't be converted, and that
    /// value as text.
    pub(crate) fn first_failure(&self, batch: &RecordBatch) -> Result<Option<(usize, String)>> {
        let input = batch
            .column_by_name(&self.column)
            .ok_or_else(|| invalid!(ColumnNotFound, column = self.column))?;
        let output = self.convert(input)?;
        Ok(failures(input, &output)
            .first()
            .map(|&row| (row, text_at(input, row))))
    }

    /// Tries the conversion on `sample` (values of the column).
    pub fn preview(&self, sample: &ArrayRef) -> CastPreview {
        let Ok(output) = self.convert(sample) else {
            return CastPreview {
                checked: sample.len(),
                failed: sample.len() - sample.null_count(),
                examples: Vec::new(),
            };
        };
        let failed = failures(sample, &output);
        let mut examples: Vec<String> = Vec::new();
        for &row in &failed {
            let text = text_at(sample, row);
            if !examples.contains(&text) {
                examples.push(text);
            }
            if examples.len() == 5 {
                break;
            }
        }
        CastPreview {
            checked: sample.len(),
            failed: failed.len(),
            examples,
        }
    }

    /// Converts values; ones that can't be converted become null.
    pub(crate) fn convert(&self, input: &ArrayRef) -> Result<ArrayRef> {
        let safe = ArrowCastOptions {
            safe: true,
            ..Default::default()
        };
        if !is_text(input.data_type()) {
            let output = cast_with_options(input, &self.to, &safe)?;
            let custom = self.options.decimal != '.' || self.options.thousands.is_some();
            if input.data_type().is_numeric() && is_text(&self.to) && custom {
                // Numbers written with the chosen separators.
                let strings = arrow::compute::cast(&output, &DataType::Utf8)?;
                let strings = strings
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| invalid!(ExpectedText))?;
                let formatted: StringArray = strings
                    .iter()
                    .map(|s| s.map(|s| self.format_number(s)))
                    .collect();
                return Ok(arrow::compute::cast(&formatted, &self.to)?);
            }
            return Ok(output);
        }
        let strings = arrow::compute::cast(input, &DataType::Utf8)?;
        let strings = strings
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| invalid!(ExpectedText))?;
        if self.to.is_nested() {
            return parse_json_column(strings, &self.to);
        }
        let numeric = self.to.is_numeric();
        if let (Some(format), true) = (
            &self.options.format,
            matches!(self.to, DataType::Date32 | DataType::Timestamp(..)),
        ) {
            return parse_dates(strings, format, &self.to);
        }
        let cleaned: StringArray = strings
            .iter()
            .map(|s| {
                s.map(|s| {
                    let s = s.trim();
                    if numeric {
                        self.clean_number(s)
                    } else {
                        s.to_owned()
                    }
                })
            })
            .collect();
        Ok(cast_with_options(&cleaned, &self.to, &safe)?)
    }

    /// `-1234567.5` → `-1.234.567,5` with a comma decimal and dot thousands.
    /// Scientific notation is left as is.
    fn format_number(&self, s: &str) -> String {
        if s.contains(['e', 'E']) || !s.chars().any(|c| c.is_ascii_digit()) {
            return s.to_owned();
        }
        let (sign, rest) = match s.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", s),
        };
        let (int, frac) = match rest.split_once('.') {
            Some((int, frac)) => (int, Some(frac)),
            None => (rest, None),
        };
        let mut out = String::from(sign);
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                out.extend(self.options.thousands);
            }
            out.push(c);
        }
        if let Some(frac) = frac {
            out.push(self.options.decimal);
            out.push_str(frac);
        }
        out
    }

    fn clean_number(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if Some(c) == self.options.thousands
                || (self.options.thousands == Some(' ') && c == '\u{a0}')
            {
                continue;
            }
            out.push(if c == self.options.decimal { '.' } else { c });
        }
        out
    }
}

/// Reads JSON text as values of the nested type `to`; text that isn't valid
/// JSON for it becomes null.
fn parse_json_column(strings: &StringArray, to: &DataType) -> Result<ArrayRef> {
    let values: Vec<ArrayRef> = strings
        .iter()
        .map(|s| {
            let parsed = s.and_then(|s| crate::values::parse_value(Some(s), to).ok());
            parsed.unwrap_or_else(|| arrow::array::new_null_array(to, 1))
        })
        .collect();
    if values.is_empty() {
        return Ok(arrow::array::new_empty_array(to));
    }
    let refs: Vec<&dyn Array> = values.iter().map(|a| a.as_ref()).collect();
    Ok(arrow::compute::concat(&refs)?)
}

/// Rows where a non-null input became null.
fn failures(input: &ArrayRef, output: &ArrayRef) -> Vec<usize> {
    (0..input.len())
        .filter(|&i| input.is_valid(i) && output.is_null(i))
        .collect()
}

fn text_at(array: &ArrayRef, row: usize) -> String {
    let slice = array.slice(row, 1);
    arrow::compute::cast(&slice, &DataType::Utf8)
        .ok()
        .and_then(|a| {
            a.as_any()
                .downcast_ref::<StringArray>()
                .filter(|s| s.is_valid(0))
                .map(|s| s.value(0).to_owned())
        })
        .unwrap_or_default()
}

// Date and time formats: a subset of strftime.

/// Parts of a date and time read from text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DateTime {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    nanos: u32,
}

/// Month names read by `%b`/`%B`: English, Spanish, French, German,
/// Italian, Portuguese and Dutch, in lowercase.
const MONTHS: [&[&str]; 12] = [
    &[
        "january", "enero", "janvier", "januar", "gennaio", "janeiro", "januari",
    ],
    &[
        "february",
        "febrero",
        "février",
        "fevrier",
        "februar",
        "febbraio",
        "fevereiro",
        "februari",
    ],
    &[
        "march", "marzo", "mars", "märz", "marz", "março", "marco", "maart",
    ],
    &["april", "abril", "avril", "aprile"],
    &["may", "mayo", "mai", "maggio", "maio", "mei"],
    &["june", "junio", "juin", "juni", "giugno", "junho"],
    &["july", "julio", "juillet", "juli", "luglio", "julho"],
    &["august", "agosto", "août", "aout", "augustus"],
    &[
        "september",
        "septiembre",
        "setiembre",
        "septembre",
        "settembre",
        "setembro",
    ],
    &[
        "october", "octubre", "octobre", "oktober", "ottobre", "outubro",
    ],
    &["november", "noviembre", "novembre", "novembro"],
    &[
        "december",
        "diciembre",
        "décembre",
        "decembre",
        "dezember",
        "dicembre",
        "dezembro",
    ],
];

/// Reads a month name at the start of `text` (lowercase): a full name, or
/// (with `abbreviated`) its first three letters, optionally followed by a
/// dot. Returns the month (1-12) and the length read in bytes.
fn month_name(text: &str, abbreviated: bool) -> Option<(u32, usize)> {
    // The longest full name wins ("marzo" over "mar").
    let full = MONTHS
        .iter()
        .enumerate()
        .flat_map(|(i, names)| names.iter().map(move |n| (i, *n)))
        .filter(|(_, name)| text.starts_with(name))
        .max_by_key(|(_, name)| name.len());
    if let Some((i, name)) = full {
        return Some((i as u32 + 1, name.len()));
    }
    if !abbreviated {
        return None;
    }
    let prefix: String = text.chars().take(3).collect();
    if prefix.chars().count() < 3 {
        return None;
    }
    // Abbreviations shared by different months ("jui": juin, juillet) don't
    // count.
    let months: Vec<usize> = MONTHS
        .iter()
        .enumerate()
        .filter(|(_, names)| names.iter().any(|n| n.starts_with(&prefix)))
        .map(|(i, _)| i)
        .collect();
    let [month] = months[..] else {
        return None;
    };
    let mut len = prefix.len();
    if text[len..].starts_with('.') {
        len += 1;
    }
    Some((month as u32 + 1, len))
}

fn check_format(format: &str) -> Result<()> {
    let mut chars = format.chars();
    let mut has_code = false;
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        match chars.next() {
            Some('Y' | 'y' | 'm' | 'd' | 'b' | 'B' | 'H' | 'I' | 'p' | 'M' | 'S' | 'f') => {
                has_code = true;
            }
            Some('%') => {}
            Some(other) => return Err(invalid!(FormatUnknownCode, code = other)),
            None => return Err(invalid!(FormatEndsWithPercent)),
        }
    }
    if !has_code {
        return Err(invalid!(FormatNoCodes));
    }
    Ok(())
}

/// Reads up to `max` digits.
fn digits(s: &mut &str, max: usize) -> Option<(u64, usize)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    let value = s[..n].parse().ok()?;
    *s = &s[n..];
    Some((value, n))
}

fn parse_datetime(text: &str, format: &str) -> Option<DateTime> {
    let mut s = text.trim();
    let mut dt = DateTime {
        year: 1970,
        month: 1,
        day: 1,
        ..DateTime::default()
    };
    let mut pm = None;
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            if c.is_whitespace() {
                s = s.trim_start();
                continue;
            }
            s = s.strip_prefix(c)?;
            continue;
        }
        match chars.next()? {
            'Y' => {
                let negative = s.starts_with('-');
                if negative {
                    s = &s[1..];
                }
                let year = digits(&mut s, 6)?.0 as i64;
                dt.year = if negative { -year } else { year };
            }
            'y' => {
                let year = digits(&mut s, 2)?.0 as i64;
                dt.year = if year < 70 { 2000 + year } else { 1900 + year };
            }
            'm' => dt.month = digits(&mut s, 2)?.0 as u32,
            'd' => dt.day = digits(&mut s, 2)?.0 as u32,
            'H' | 'I' => dt.hour = digits(&mut s, 2)?.0 as u32,
            'M' => dt.minute = digits(&mut s, 2)?.0 as u32,
            'S' => dt.second = digits(&mut s, 2)?.0 as u32,
            'f' => {
                let (value, n) = digits(&mut s, 9)?;
                dt.nanos = (value * 10u64.pow(9 - n as u32)) as u32;
            }
            code @ ('b' | 'B') => {
                let lower = s.to_lowercase();
                // Lowercasing can change byte lengths; only accept text
                // where it doesn't.
                if lower.len() != s.len() {
                    return None;
                }
                let (month, len) = month_name(&lower, code == 'b')?;
                dt.month = month;
                s = s.get(len..)?;
            }
            'p' => {
                let lower = s.get(..2)?.to_lowercase();
                pm = Some(match lower.as_str() {
                    "am" => false,
                    "pm" => true,
                    _ => return None,
                });
                s = &s[2..];
            }
            '%' => s = s.strip_prefix('%')?,
            _ => return None,
        }
    }
    if !s.is_empty() {
        return None;
    }
    if let Some(pm) = pm {
        if !(1..=12).contains(&dt.hour) {
            return None;
        }
        dt.hour = dt.hour % 12 + if pm { 12 } else { 0 };
    }
    let valid = (1..=12).contains(&dt.month)
        && dt.day >= 1
        && dt.day <= days_in_month(dt.year, dt.month)
        && dt.hour < 24
        && dt.minute < 60
        && dt.second < 60;
    valid.then_some(dt)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 (proleptic Gregorian).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn parse_dates(strings: &StringArray, format: &str, to: &DataType) -> Result<ArrayRef> {
    let parsed = strings
        .iter()
        .map(|s| s.and_then(|s| parse_datetime(s, format)));
    let nanos = |dt: DateTime| -> Option<i128> {
        let days = days_from_civil(dt.year, dt.month, dt.day) as i128;
        let seconds =
            days * 86_400 + (dt.hour as i128) * 3600 + (dt.minute as i128) * 60 + dt.second as i128;
        Some(seconds * 1_000_000_000 + dt.nanos as i128)
    };
    let scaled = |divisor: i128| {
        parsed
            .clone()
            .map(|dt| {
                dt.and_then(nanos)
                    .and_then(|n| i64::try_from(n / divisor).ok())
            })
            .collect::<Vec<_>>()
    };
    Ok(match to {
        DataType::Date32 => Arc::new(Date32Array::from_iter(parsed.clone().map(|dt| {
            dt.and_then(|dt| i32::try_from(days_from_civil(dt.year, dt.month, dt.day)).ok())
        }))),
        DataType::Timestamp(unit, tz) => {
            let array: ArrayRef = match unit {
                TimeUnit::Second => Arc::new(TimestampSecondArray::from(scaled(1_000_000_000))),
                TimeUnit::Millisecond => {
                    Arc::new(TimestampMillisecondArray::from(scaled(1_000_000)))
                }
                TimeUnit::Microsecond => Arc::new(TimestampMicrosecondArray::from(scaled(1_000))),
                TimeUnit::Nanosecond => Arc::new(TimestampNanosecondArray::from(scaled(1))),
            };
            // Attach the time zone, if any.
            arrow::compute::cast(&array, &DataType::Timestamp(*unit, tz.clone()))?
        }
        other => {
            return Err(invalid!(FormatNotForType, data_type = type_name(other)));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float64Array, Int64Array};

    fn text(values: &[Option<&str>]) -> ArrayRef {
        Arc::new(StringArray::from(values.to_vec()))
    }

    fn cast(to: DataType, options: CastOptions) -> Cast {
        Cast {
            column: "c".into(),
            to,
            options,
        }
    }

    #[test]
    fn numbers_with_locale_separators() {
        let c = cast(
            DataType::Float64,
            CastOptions {
                decimal: ',',
                thousands: Some('.'),
                ..CastOptions::default()
            },
        );
        let out = c
            .convert(&text(&[Some("1.234,5"), Some(" 7 "), Some("x"), None]))
            .unwrap();
        let out = out.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!(
            out.iter().collect::<Vec<_>>(),
            [Some(1234.5), Some(7.0), None, None]
        );

        let preview = c.preview(&text(&[Some("1,5"), Some("x"), Some("y"), Some("x"), None]));
        assert_eq!(
            preview,
            CastPreview {
                checked: 5,
                failed: 3,
                examples: vec!["x".into(), "y".into()]
            }
        );
    }

    #[test]
    fn dates_with_formats() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        let parse = |s, f| parse_datetime(s, f);
        assert_eq!(parse("31/12/2024", "%d/%m/%Y").unwrap().day, 31);
        assert!(parse("31/02/2024", "%d/%m/%Y").is_none());
        assert!(parse("29/02/2024", "%d/%m/%Y").is_some());
        assert!(parse("12/31/2024x", "%m/%d/%Y").is_none());
        let dt = parse("5-Mar-24 07:08:09.25 PM", "%d-%b-%y %I:%M:%S.%f %p").unwrap();
        assert_eq!(
            (dt.year, dt.month, dt.day, dt.hour, dt.nanos),
            (2024, 3, 5, 19, 250_000_000)
        );

        let c = cast(
            DataType::Timestamp(TimeUnit::Microsecond, None),
            CastOptions {
                format: Some("%d.%m.%Y %H:%M".into()),
                ..CastOptions::default()
            },
        );
        let out = c
            .convert(&text(&[Some("02.01.1970 00:01"), Some("bad")]))
            .unwrap();
        let out = out
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(
            out.iter().collect::<Vec<_>>(),
            [Some((86_400 + 60) * 1_000_000), None]
        );
        assert!(check_format("%d/%q").is_err());
        assert!(check_format("dd/mm").is_err());
    }

    #[test]
    fn error_modes() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("c", DataType::Utf8, true),
            Field::new("d", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                text(&[Some("1"), Some("two"), None]),
                Arc::new(Int64Array::from(vec![1, 2, 3])),
            ],
        )
        .unwrap();
        let run = |on_error| {
            let c = cast(
                DataType::Int64,
                CastOptions {
                    on_error,
                    ..CastOptions::default()
                },
            );
            let out = Arc::new(Schema::new(c.output_fields(&schema).unwrap()));
            c.apply(&batch, &out)
        };
        let nulls = run(OnError::Null).unwrap();
        assert_eq!(nulls.column(0).null_count(), 2);
        assert!(
            run(OnError::Fail)
                .unwrap_err()
                .to_string()
                .contains("\"two\"")
        );
        let errors = run(OnError::ErrorColumn).unwrap();
        assert_eq!(errors.schema().field(1).name(), "c errors");
        let column = errors
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(column.iter().collect::<Vec<_>>(), [None, Some("two"), None]);

        // Non-text columns use arrow's conversions.
        let to_text = cast(DataType::Utf8, CastOptions::default());
        let fields = to_text.output_fields(&schema);
        assert!(fields.is_ok());
        let bad = Cast {
            column: "d".into(),
            to: DataType::Date32,
            options: CastOptions {
                format: Some("%Y".into()),
                ..CastOptions::default()
            },
        };
        assert!(bad.output_fields(&schema).is_err());
        let wrong_target = cast(
            DataType::Int64,
            CastOptions {
                format: Some("%Y".into()),
                ..CastOptions::default()
            },
        );
        assert!(wrong_target.output_fields(&schema).is_err());
    }

    #[test]
    fn numbers_to_text_with_separators() {
        let numbers: ArrayRef = Arc::new(arrow::array::Float64Array::from(vec![
            Some(-1234567.5),
            Some(12.0),
            None,
        ]));
        let c = cast(
            DataType::Utf8,
            CastOptions {
                decimal: ',',
                thousands: Some('.'),
                ..CastOptions::default()
            },
        );
        let out = c.convert(&numbers).unwrap();
        let out = out.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(
            out.iter().collect::<Vec<_>>(),
            [Some("-1.234.567,5"), Some("12,0"), None]
        );
    }

    #[test]
    fn month_names_in_several_languages() {
        let dates = text(&[
            Some("3 marzo 2024"),
            Some("3 mars 2024"),
            Some("3 Okt. 2024"),
            Some("3 dic 2024"),
            Some("3 juin 2024"),
            Some("3 jui 2024"), // juin or juillet: ambiguous
        ]);
        let c = cast(
            DataType::Date32,
            CastOptions {
                format: Some("%d %b %Y".into()),
                ..CastOptions::default()
            },
        );
        let out = c.convert(&dates).unwrap();
        let out = arrow::compute::cast(&out, &DataType::Utf8).unwrap();
        let out = out.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(
            out.iter().collect::<Vec<_>>(),
            [
                Some("2024-03-03"),
                Some("2024-03-03"),
                Some("2024-10-03"),
                Some("2024-12-03"),
                Some("2024-06-03"),
                None
            ]
        );
    }

    #[test]
    fn text_to_nested_and_more_type_names() {
        use crate::display::parse_type_name;
        let to = parse_type_name("list<int64>").unwrap();
        let c = cast(to.clone(), CastOptions::default());
        let input = text(&[Some("[1, 2]"), Some("nope"), None]);
        let schema = Schema::new(vec![Field::new("c", DataType::Utf8, true)]);
        assert!(c.output_fields(&schema).is_ok());
        let out = c.convert(&input).unwrap();
        assert_eq!(out.data_type(), &to);
        assert_eq!(out.null_count(), 2);
        assert_eq!(
            parse_type_name("duration[ms]"),
            Some(DataType::Duration(TimeUnit::Millisecond))
        );
        assert_eq!(
            parse_type_name("decimal128(10, 2)"),
            Some(DataType::Decimal128(10, 2))
        );
        assert_eq!(
            parse_type_name("timestamp[ms, Europe/Madrid]"),
            Some(DataType::Timestamp(
                TimeUnit::Millisecond,
                Some("Europe/Madrid".into())
            ))
        );
    }
}
