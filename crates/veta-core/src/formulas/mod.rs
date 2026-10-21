//! A small expression language for computed columns.
//!
//! ```text
//! [Unit price] * [Quantity]          column references in brackets
//! price * 1.21                       or bare, for simple names
//! if(score >= 5, "pass", "fail")     functions
//! upper(left(name, 1)) & "."         & joins text
//! year(date) = 2024 and not flag     comparisons and logic
//! ```
//!
//! Expressions are type-checked against the input schema (so the output
//! column's type is known without reading data) and evaluated on whole
//! batches with arrow kernels.

mod eval;
mod functions;
mod parser;

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, NullArray, StringArray, new_null_array,
};
use arrow::compute::kernels::cmp;
use arrow::compute::kernels::numeric::{add_wrapping, div, mul_wrapping, sub_wrapping};
use arrow::compute::kernels::temporal::{DatePart, date_part};
use arrow::compute::kernels::zip::zip;
use arrow::compute::{and_kleene, cast, is_not_null, not, or_kleene, prep_null_mask_filter};
use arrow::datatypes::{DataType, Schema};
use arrow::record_batch::RecordBatch;

use crate::display::type_name;
use crate::error::Result;
use crate::steps::invalid;
use crate::values::is_text;

use eval::{Context, to};
pub use functions::{expression_help, function_signatures};
pub use parser::parse;

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column(String),
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Null,
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

// Types.

/// The types expressions compute with. Columns are read as one of these.
fn normalize(data_type: &DataType) -> DataType {
    match data_type {
        t if t.is_integer() => DataType::Int64,
        t if t.is_floating() => DataType::Float64,
        DataType::Decimal128(..) | DataType::Decimal256(..) => DataType::Float64,
        t if is_text(t) => DataType::Utf8,
        DataType::Dictionary(_, value) => normalize(value),
        other => other.clone(),
    }
}

fn is_number(t: &DataType) -> bool {
    matches!(t, DataType::Int64 | DataType::Float64 | DataType::Null)
}

fn is_temporal(t: &DataType) -> bool {
    matches!(
        t,
        DataType::Date32 | DataType::Date64 | DataType::Timestamp(..) | DataType::Null
    )
}

/// The common type of two values (numbers widen to float; null fits all).
fn unify(a: &DataType, b: &DataType) -> Option<DataType> {
    match (a, b) {
        (DataType::Null, t) | (t, DataType::Null) => Some(t.clone()),
        (a, b) if a == b => Some(a.clone()),
        (a, b) if is_number(a) && is_number(b) => Some(DataType::Float64),
        _ => None,
    }
}

fn mismatch(what: &str, a: &DataType, b: &DataType) -> crate::error::Error {
    invalid(format!(
        "can't {what} {} and {}",
        type_name(a),
        type_name(b)
    ))
}

fn expect(what: &str, t: &DataType, ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("{what}, not {}", type_name(t))))
    }
}

impl Expr {
    /// The type of the expression's values over `schema`.
    pub fn data_type(&self, schema: &Schema) -> Result<DataType> {
        Ok(match self {
            Expr::Column(name) => normalize(
                schema
                    .field_with_name(name)
                    .map_err(|_| invalid(format!("no column named {name:?}")))?
                    .data_type(),
            ),
            Expr::Int(_) => DataType::Int64,
            Expr::Float(_) => DataType::Float64,
            Expr::Text(_) => DataType::Utf8,
            Expr::Bool(_) => DataType::Boolean,
            Expr::Null => DataType::Null,
            Expr::Neg(e) => {
                let t = e.data_type(schema)?;
                expect("- needs a number", &t, is_number(&t))?;
                t
            }
            Expr::Not(e) => {
                let t = e.data_type(schema)?;
                expect(
                    "not needs true/false",
                    &t,
                    matches!(t, DataType::Boolean | DataType::Null),
                )?;
                DataType::Boolean
            }
            Expr::Binary(op, a, b) => {
                let (ta, tb) = (a.data_type(schema)?, b.data_type(schema)?);
                match op {
                    BinOp::Or | BinOp::And => {
                        let ok = |t: &DataType| matches!(t, DataType::Boolean | DataType::Null);
                        if !ok(&ta) || !ok(&tb) {
                            return Err(mismatch("combine with and/or", &ta, &tb));
                        }
                        DataType::Boolean
                    }
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        if unify(&ta, &tb).is_none() {
                            return Err(mismatch("compare", &ta, &tb));
                        }
                        DataType::Boolean
                    }
                    BinOp::Concat => DataType::Utf8,
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Rem => {
                        if !is_number(&ta) || !is_number(&tb) {
                            return Err(mismatch("do arithmetic on", &ta, &tb));
                        }
                        if ta == DataType::Float64 || tb == DataType::Float64 {
                            DataType::Float64
                        } else {
                            DataType::Int64
                        }
                    }
                    BinOp::Div => {
                        if !is_number(&ta) || !is_number(&tb) {
                            return Err(mismatch("divide", &ta, &tb));
                        }
                        DataType::Float64
                    }
                }
            }
            Expr::Call(name, args) => {
                let types = args
                    .iter()
                    .map(|a| a.data_type(schema))
                    .collect::<Result<Vec<_>>>()?;
                let text_arg = |i: usize| -> Result<()> {
                    let t = &types[i];
                    expect(
                        &format!("{name} needs text"),
                        t,
                        matches!(t, DataType::Utf8 | DataType::Null),
                    )
                };
                let number_arg = |i: usize| -> Result<()> {
                    let t = &types[i];
                    expect(&format!("{name} needs a number"), t, is_number(t))
                };
                match name.as_str() {
                    "if" => {
                        expect(
                            "if needs a true/false condition",
                            &types[0],
                            matches!(types[0], DataType::Boolean | DataType::Null),
                        )?;
                        unify(&types[1], &types[2])
                            .ok_or_else(|| mismatch("choose between", &types[1], &types[2]))?
                    }
                    "coalesce" => {
                        let mut t = DataType::Null;
                        for other in &types {
                            t = unify(&t, other).ok_or_else(|| mismatch("coalesce", &t, other))?;
                        }
                        t
                    }
                    "text" => DataType::Utf8,
                    "number" => DataType::Float64,
                    "upper" | "lower" | "trim" => {
                        text_arg(0)?;
                        DataType::Utf8
                    }
                    "len" => {
                        text_arg(0)?;
                        DataType::Int64
                    }
                    "left" | "right" => {
                        text_arg(0)?;
                        number_arg(1)?;
                        DataType::Utf8
                    }
                    "mid" => {
                        text_arg(0)?;
                        number_arg(1)?;
                        number_arg(2)?;
                        DataType::Utf8
                    }
                    "contains" => {
                        text_arg(0)?;
                        text_arg(1)?;
                        DataType::Boolean
                    }
                    "abs" => {
                        number_arg(0)?;
                        types[0].clone()
                    }
                    "round" | "floor" | "ceil" => {
                        number_arg(0)?;
                        if types.len() > 1 {
                            number_arg(1)?;
                        }
                        DataType::Float64
                    }
                    "year" | "month" | "day" | "hour" | "minute" | "second" => {
                        let t = &types[0];
                        expect(&format!("{name} needs a date or time"), t, is_temporal(t))?;
                        DataType::Int64
                    }
                    "row" => DataType::Int64,
                    other => return Err(invalid(format!("unknown function {other}"))),
                }
            }
        })
    }
}

/// A new column computed from an expression (Power Query's "Custom
/// Column").
#[derive(Debug, Clone, PartialEq)]
pub struct CustomColumn {
    pub name: String,
    pub expression: String,
    /// Position of the new column.
    pub at: usize,
}

impl CustomColumn {
    pub fn describe(&self) -> String {
        format!("Added column {} = {}", self.name, self.expression)
    }

    /// The new column's type: a plain column reference keeps the column's
    /// type (duplicating it); text when the expression is always null.
    pub fn data_type(&self, schema: &Schema) -> Result<DataType> {
        let expr = parse(&self.expression)?;
        if let Expr::Column(name) = &expr
            && let Ok(field) = schema.field_with_name(name)
        {
            return Ok(field.data_type().clone());
        }
        let t = expr.data_type(schema)?;
        Ok(if t == DataType::Null {
            DataType::Utf8
        } else {
            t
        })
    }

    pub(crate) fn output_fields(&self, schema: &Schema) -> Result<Vec<arrow::datatypes::Field>> {
        if self.name.trim().is_empty() {
            return Err(invalid("column name cannot be empty".into()));
        }
        if schema.fields().iter().any(|f| f.name() == &self.name) {
            return Err(invalid(format!(
                "a column named {:?} already exists",
                self.name
            )));
        }
        if self.at > schema.fields().len() {
            return Err(invalid(format!(
                "column position {} is out of range",
                self.at
            )));
        }
        let t = self.data_type(schema)?;
        let mut fields: Vec<arrow::datatypes::Field> =
            schema.fields().iter().map(|f| f.as_ref().clone()).collect();
        fields.insert(self.at, arrow::datatypes::Field::new(&self.name, t, true));
        Ok(fields)
    }

    pub(crate) fn apply(
        &self,
        batch: &RecordBatch,
        first_row: usize,
        schema: &Arc<Schema>,
    ) -> Result<RecordBatch> {
        let expr = parse(&self.expression)?;
        let value = expr.eval(&Context { batch, first_row })?;
        let value = to(&value, schema.field(self.at).data_type())?;
        let mut columns = batch.columns().to_vec();
        columns.insert(self.at, value);
        Ok(RecordBatch::try_new(schema.clone(), columns)?)
    }
}

#[cfg(test)]
mod tests {
    use super::eval::*;
    use super::*;
    use arrow::array::Date32Array;
    use arrow::datatypes::Field;

    fn batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("Unit price", DataType::Float64, true),
            Field::new("qty", DataType::Int32, true),
            Field::new("name", DataType::Utf8, true),
            Field::new("date", DataType::Date32, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Float64Array::from(vec![Some(2.5), Some(1.0), None])),
                Arc::new(arrow::array::Int32Array::from(vec![
                    Some(4),
                    Some(3),
                    Some(7),
                ])),
                Arc::new(StringArray::from(vec![Some(" Ana "), None, Some("bob")])),
                Arc::new(Date32Array::from(vec![Some(19_723), Some(0), None])),
            ],
        )
        .unwrap()
    }

    fn eval(source: &str) -> ArrayRef {
        let batch = batch();
        let expr = parse(source).unwrap();
        let t = expr.data_type(&batch.schema()).unwrap();
        let out = expr
            .eval(&Context {
                batch: &batch,
                first_row: 10,
            })
            .unwrap();
        assert_eq!(out.data_type(), &t, "{source}");
        out
    }

    fn text(source: &str) -> Vec<Option<String>> {
        strings(&eval(source))
            .unwrap()
            .iter()
            .map(|s| s.map(str::to_owned))
            .collect()
    }

    fn nums(source: &str) -> Vec<Option<f64>> {
        floats(&eval(source)).unwrap().iter().collect()
    }

    #[test]
    fn parses_with_precedence() {
        assert_eq!(
            parse("1 + 2 * 3").unwrap(),
            Expr::Binary(
                BinOp::Add,
                Box::new(Expr::Int(1)),
                Box::new(Expr::Binary(
                    BinOp::Mul,
                    Box::new(Expr::Int(2)),
                    Box::new(Expr::Int(3))
                ))
            )
        );
        assert!(matches!(
            parse("not a = b or c").unwrap(),
            Expr::Binary(BinOp::Or, ..)
        ));
        assert!(matches!(
            parse("[a b] <> \"x\"\"y\"").unwrap(),
            Expr::Binary(BinOp::Ne, ..)
        ));
        for bad in [
            "", "1 +", "(1", "foo(1)", "left(a)", "[a", "\"x", "1 2", "a $ b",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn arithmetic_and_types() {
        assert_eq!(nums("[Unit price] * qty"), [Some(10.0), Some(3.0), None]);
        assert_eq!(ints(&eval("qty % 2 + -1")).unwrap().values(), &[-1, 0, 0]);
        assert_eq!(nums("qty / 2"), [Some(2.0), Some(1.5), Some(3.5)]);
        assert_eq!(nums("round(qty / 3, 2)")[0], Some(1.33));
        let schema = batch().schema();
        assert!(parse("name + 1").unwrap().data_type(&schema).is_err());
        assert!(parse("upper(qty)").unwrap().data_type(&schema).is_err());
        assert!(parse("nope * 2").unwrap().data_type(&schema).is_err());
    }

    #[test]
    fn text_logic_and_dates() {
        assert_eq!(
            text("upper(trim(name)) & \"!\""),
            [Some("ANA!".into()), None, Some("BOB!".into())]
        );
        assert_eq!(
            text("if(qty > 3 and not contains(coalesce(name, \"\"), \"o\"), \"big\", \"small\")"),
            [
                Some("big".into()),
                Some("small".into()),
                Some("small".into())
            ]
        );
        assert_eq!(
            text("left(name, 2) & mid(\"hello\", 2, 3) & right(\"abc\", 1)")[2].as_deref(),
            Some("boellc")
        );
        assert_eq!(
            ints(&eval("year(date)"))
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [Some(2024), Some(1970), None]
        );
        assert_eq!(ints(&eval("row()")).unwrap().values(), &[11, 12, 13]);
        assert_eq!(
            ints(&eval("len(name)")).unwrap().iter().collect::<Vec<_>>(),
            [Some(5), None, Some(3)]
        );
        let nulls = eval("if(qty > 100, null, null)");
        assert_eq!(nulls.logical_null_count(), 3);
    }
}
