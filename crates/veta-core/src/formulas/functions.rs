//! Built-in functions: their names and argument counts, and how each is
//! evaluated. An Excel-like function library would grow here.

use super::eval::*;
use super::*;

/// Functions, with their argument counts (min, max).
pub(super) const FUNCTIONS: &[(&str, usize, usize, &str)] = &[
    ("if", 3, 3, "if(condition, then, else)"),
    (
        "coalesce",
        1,
        usize::MAX,
        "coalesce(a, b, …): first non-null",
    ),
    ("text", 1, 1, "text(x)"),
    ("number", 1, 1, "number(x)"),
    ("upper", 1, 1, "upper(s)"),
    ("lower", 1, 1, "lower(s)"),
    ("trim", 1, 1, "trim(s)"),
    ("len", 1, 1, "len(s)"),
    ("left", 2, 2, "left(s, n)"),
    ("right", 2, 2, "right(s, n)"),
    ("mid", 3, 3, "mid(s, start, n)"),
    ("contains", 2, 2, "contains(s, part)"),
    ("abs", 1, 1, "abs(x)"),
    ("round", 1, 2, "round(x, digits)"),
    ("floor", 1, 1, "floor(x)"),
    ("ceil", 1, 1, "ceil(x)"),
    ("year", 1, 1, "year(d)"),
    ("month", 1, 1, "month(d)"),
    ("day", 1, 1, "day(d)"),
    ("hour", 1, 1, "hour(t)"),
    ("minute", 1, 1, "minute(t)"),
    ("second", 1, 1, "second(t)"),
    ("row", 0, 0, "row(): row number from 1"),
];

/// Short help listing the syntax and functions.
pub fn expression_help() -> String {
    let functions: Vec<&str> = FUNCTIONS.iter().map(|f| f.3).collect();
    format!(
        "Columns: [Name] or name. Operators: + - * / % & (join text) = <> < <= > >= and or not. \
         Functions: {}.",
        functions.join(", ")
    )
}

impl Expr {
    pub(super) fn call(&self, name: &str, args: &[Expr], cx: &Context<'_>) -> Result<ArrayRef> {
        let n = cx.len();
        if name == "row" {
            let first = cx.first_row as i64 + 1;
            return Ok(Arc::new(Int64Array::from_iter_values(
                first..first + n as i64,
            )));
        }
        let values = args
            .iter()
            .map(|a| a.eval(cx))
            .collect::<Result<Vec<_>>>()?;
        let v = |i: usize| &values[i];
        Ok(match name {
            "if" => {
                let t = unify(v(1).data_type(), v(2).data_type()).ok_or_else(|| {
                    mismatch("choose between", v(1).data_type(), v(2).data_type())
                })?;
                if t == DataType::Null {
                    return Ok(Arc::new(NullArray::new(n)));
                }
                zip(&condition(v(0))?, &to(v(1), &t)?, &to(v(2), &t)?)?
            }
            "coalesce" => {
                let mut t = DataType::Null;
                for value in &values {
                    t = unify(&t, value.data_type())
                        .ok_or_else(|| mismatch("coalesce", &t, value.data_type()))?;
                }
                if t == DataType::Null {
                    return Ok(Arc::new(NullArray::new(n)));
                }
                let mut result = to(v(values.len() - 1), &t)?;
                for value in values.iter().rev().skip(1) {
                    let value = to(value, &t)?;
                    result = zip(&is_not_null(&value)?, &value, &result)?;
                }
                result
            }
            "text" => to(v(0), &DataType::Utf8)?,
            "number" => {
                let options = arrow::compute::CastOptions {
                    safe: true,
                    ..Default::default()
                };
                arrow::compute::cast_with_options(v(0), &DataType::Float64, &options)?
            }
            "upper" => map_text(v(0), str::to_uppercase)?,
            "lower" => map_text(v(0), str::to_lowercase)?,
            "trim" => map_text(v(0), |s| s.trim().to_owned())?,
            "len" => {
                let s = strings(v(0))?;
                Arc::new(
                    s.iter()
                        .map(|s| s.map(|s| s.chars().count() as i64))
                        .collect::<Int64Array>(),
                )
            }
            "left" | "right" | "mid" => {
                let s = strings(v(0))?;
                let a = ints(v(1))?;
                let b = if name == "mid" {
                    Some(ints(v(2))?)
                } else {
                    None
                };
                Arc::new(
                    (0..n)
                        .map(|i| {
                            if s.is_null(i)
                                || a.is_null(i)
                                || b.as_ref().is_some_and(|b| b.is_null(i))
                            {
                                return None;
                            }
                            let text = s.value(i);
                            let count = text.chars().count();
                            let (skip, take) = match name {
                                "left" => (0, a.value(i).max(0) as usize),
                                "right" => {
                                    let k = (a.value(i).max(0) as usize).min(count);
                                    (count - k, k)
                                }
                                // mid: 1-based start.
                                _ => (
                                    (a.value(i).max(1) - 1) as usize,
                                    b.as_ref().map_or(0, |b| b.value(i).max(0) as usize),
                                ),
                            };
                            Some(text.chars().skip(skip).take(take).collect::<String>())
                        })
                        .collect::<StringArray>(),
                )
            }
            "contains" => {
                let (s, part) = (strings(v(0))?, strings(v(1))?);
                Arc::new(
                    s.iter()
                        .zip(part.iter())
                        .map(|(s, p)| Some(s?.contains(p?)))
                        .collect::<BooleanArray>(),
                )
            }
            "abs" => match v(0).data_type() {
                DataType::Int64 => Arc::new(
                    ints(v(0))?
                        .iter()
                        .map(|x| x.map(i64::wrapping_abs))
                        .collect::<Int64Array>(),
                ),
                _ => Arc::new(
                    floats(v(0))?
                        .iter()
                        .map(|x| x.map(f64::abs))
                        .collect::<Float64Array>(),
                ),
            },
            "round" | "floor" | "ceil" => {
                let x = floats(v(0))?;
                let digits = if values.len() > 1 {
                    Some(ints(v(1))?)
                } else {
                    None
                };
                Arc::new(
                    (0..n)
                        .map(|i| {
                            if x.is_null(i) {
                                return None;
                            }
                            let d = match &digits {
                                Some(d) if d.is_null(i) => return None,
                                Some(d) => d.value(i).clamp(-15, 15) as i32,
                                None => 0,
                            };
                            let scale = 10f64.powi(d);
                            let scaled = x.value(i) * scale;
                            let r = match name {
                                "round" => scaled.round(),
                                "floor" => scaled.floor(),
                                _ => scaled.ceil(),
                            };
                            Some(r / scale)
                        })
                        .collect::<Float64Array>(),
                )
            }
            "year" | "month" | "day" | "hour" | "minute" | "second" => {
                if v(0).data_type() == &DataType::Null {
                    return Ok(new_null_array(&DataType::Int64, n));
                }
                let part = match name {
                    "year" => DatePart::Year,
                    "month" => DatePart::Month,
                    "day" => DatePart::Day,
                    "hour" => DatePart::Hour,
                    "minute" => DatePart::Minute,
                    _ => DatePart::Second,
                };
                to(&date_part(v(0).as_ref(), part)?, &DataType::Int64)?
            }
            other => return Err(invalid(format!("unknown function {other}"))),
        })
    }
}
