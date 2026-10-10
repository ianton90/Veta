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

use super::invalid;
use crate::display::type_name;
use crate::error::Result;
use crate::values::is_text;

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

/// Functions, with their argument counts (min, max).
const FUNCTIONS: &[(&str, usize, usize, &str)] = &[
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

// Lexing.

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(String),
    Text(String),
    Column(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn lex(source: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let token = match c {
            '(' => Token::LParen,
            ')' => Token::RParen,
            ',' => Token::Comma,
            '[' => {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == ']')
                    .ok_or_else(|| invalid("missing ] after a column name".into()))?;
                let name: String = chars[i + 1..i + end].iter().collect();
                i += end;
                Token::Column(name)
            }
            '"' => {
                let mut text = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(invalid("missing closing \"".into())),
                        Some('"') if chars.get(i + 1) == Some(&'"') => {
                            text.push('"');
                            i += 1;
                        }
                        Some('"') => break,
                        Some(&c) => text.push(c),
                    }
                    i += 1;
                }
                Token::Text(text)
            }
            c if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                while chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    i += 1;
                }
                Token::Number(chars[start..=i].iter().collect())
            }
            c if c.is_alphabetic() || c == '_' => {
                while chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_')
                {
                    i += 1;
                }
                Token::Ident(chars[start..=i].iter().collect())
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let op = ["<=", ">=", "<>", "!="]
                    .into_iter()
                    .find(|op| *op == two)
                    .or_else(|| {
                        ["+", "-", "*", "/", "%", "&", "=", "<", ">"]
                            .into_iter()
                            .find(|op| op.starts_with(c))
                    })
                    .ok_or_else(|| invalid(format!("unexpected {c:?}")))?;
                i += op.len() - 1;
                Token::Op(if op == "!=" { "<>" } else { op })
            }
        };
        tokens.push(token);
        i += 1;
    }
    Ok(tokens)
}

// Parsing (precedence climbing).

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(w)) if w.eq_ignore_ascii_case(word))
    }

    fn binary_op(&self) -> Option<(BinOp, u8)> {
        let op = match self.peek()? {
            Token::Ident(w) if w.eq_ignore_ascii_case("or") => BinOp::Or,
            Token::Ident(w) if w.eq_ignore_ascii_case("and") => BinOp::And,
            Token::Op(op) => match *op {
                "=" => BinOp::Eq,
                "<>" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                "&" => BinOp::Concat,
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "%" => BinOp::Rem,
                _ => return None,
            },
            _ => return None,
        };
        let precedence = match op {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
            BinOp::Concat => 5,
            BinOp::Add | BinOp::Sub => 6,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 7,
        };
        Some((op, precedence))
    }

    fn expression(&mut self, min: u8) -> Result<Expr> {
        let mut left = self.unary()?;
        while let Some((op, precedence)) = self.binary_op() {
            if precedence < min {
                break;
            }
            self.next();
            let right = self.expression(precedence + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.keyword("not") {
            self.next();
            // Binds looser than comparisons: `not a = b` is `not (a = b)`.
            return Ok(Expr::Not(Box::new(self.expression(3)?)));
        }
        if self.peek() == Some(&Token::Op("-")) {
            self.next();
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr> {
        match self.next() {
            Some(Token::Number(n)) => {
                if n.contains('.') {
                    n.parse()
                        .map(Expr::Float)
                        .map_err(|_| invalid(format!("{n} is not a number")))
                } else {
                    n.parse()
                        .map(Expr::Int)
                        .map_err(|_| invalid(format!("{n} is too large")))
                }
            }
            Some(Token::Text(t)) => Ok(Expr::Text(t)),
            Some(Token::Column(c)) => Ok(Expr::Column(c)),
            Some(Token::LParen) => {
                let inner = self.expression(0)?;
                match self.next() {
                    Some(Token::RParen) => Ok(inner),
                    _ => Err(invalid("missing )".into())),
                }
            }
            Some(Token::Ident(word)) => {
                let lower = word.to_lowercase();
                if self.peek() == Some(&Token::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if self.peek() == Some(&Token::RParen) {
                        self.next();
                    } else {
                        loop {
                            args.push(self.expression(0)?);
                            match self.next() {
                                Some(Token::Comma) => {}
                                Some(Token::RParen) => break,
                                _ => return Err(invalid(format!("missing ) after {word}("))),
                            }
                        }
                    }
                    let Some(&(_, min, max, usage)) =
                        FUNCTIONS.iter().find(|(name, ..)| *name == lower)
                    else {
                        return Err(invalid(format!("unknown function {word}")));
                    };
                    if args.len() < min || args.len() > max {
                        return Err(invalid(format!("use {usage}")));
                    }
                    return Ok(Expr::Call(lower, args));
                }
                Ok(match lower.as_str() {
                    "true" => Expr::Bool(true),
                    "false" => Expr::Bool(false),
                    "null" => Expr::Null,
                    _ => Expr::Column(word),
                })
            }
            Some(Token::Op(op)) => Err(invalid(format!("unexpected {op}"))),
            Some(Token::RParen) => Err(invalid("unexpected )".into())),
            Some(Token::Comma) => Err(invalid("unexpected ,".into())),
            None => Err(invalid("the expression is incomplete".into())),
        }
    }
}

/// Parses an expression.
pub fn parse(source: &str) -> Result<Expr> {
    let tokens = lex(source)?;
    if tokens.is_empty() {
        return Err(invalid("enter an expression".into()));
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.expression(0)?;
    if let Some(token) = parser.peek() {
        return Err(invalid(format!(
            "unexpected {token:?} after the expression"
        )));
    }
    Ok(expr)
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

// Evaluation.

/// What an expression is evaluated over.
pub(super) struct Context<'a> {
    pub batch: &'a RecordBatch,
    /// Position of the batch's first row in the step's input.
    pub first_row: usize,
}

impl Context<'_> {
    fn len(&self) -> usize {
        self.batch.num_rows()
    }
}

fn to(array: &ArrayRef, t: &DataType) -> Result<ArrayRef> {
    if array.data_type() == t {
        Ok(array.clone())
    } else {
        Ok(cast(array, t)?)
    }
}

fn floats(array: &ArrayRef) -> Result<Float64Array> {
    let array = to(array, &DataType::Float64)?;
    Ok(array
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| invalid("expected a number".into()))?
        .clone())
}

fn ints(array: &ArrayRef) -> Result<Int64Array> {
    let array = to(array, &DataType::Int64)?;
    Ok(array
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| invalid("expected a whole number".into()))?
        .clone())
}

fn strings(array: &ArrayRef) -> Result<StringArray> {
    let array = to(array, &DataType::Utf8)?;
    Ok(array
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| invalid("expected text".into()))?
        .clone())
}

fn bools(array: &ArrayRef) -> Result<BooleanArray> {
    let array = to(array, &DataType::Boolean)?;
    Ok(array
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| invalid("expected true/false".into()))?
        .clone())
}

/// Nulls in a condition count as false.
fn condition(array: &ArrayRef) -> Result<BooleanArray> {
    let mask = bools(array)?;
    Ok(if mask.nulls().is_some() {
        prep_null_mask_filter(&mask)
    } else {
        mask
    })
}

fn map_text(array: &ArrayRef, f: impl Fn(&str) -> String) -> Result<ArrayRef> {
    let s = strings(array)?;
    Ok(Arc::new(
        s.iter().map(|v| v.map(&f)).collect::<StringArray>(),
    ))
}

impl Expr {
    /// Evaluates the expression; the result has [`Expr::data_type`]'s type.
    pub(super) fn eval(&self, cx: &Context<'_>) -> Result<ArrayRef> {
        let n = cx.len();
        Ok(match self {
            Expr::Column(name) => {
                let column = cx
                    .batch
                    .column_by_name(name)
                    .ok_or_else(|| invalid(format!("no column named {name:?}")))?;
                to(column, &normalize(column.data_type()))?
            }
            Expr::Int(v) => Arc::new(Int64Array::from_value(*v, n)),
            Expr::Float(v) => Arc::new(Float64Array::from_value(*v, n)),
            Expr::Text(v) => Arc::new(StringArray::from_iter_values(std::iter::repeat_n(v, n))),
            Expr::Bool(v) => Arc::new(BooleanArray::from(vec![*v; n])),
            Expr::Null => Arc::new(NullArray::new(n)),
            Expr::Neg(e) => {
                let v = e.eval(cx)?;
                match v.data_type() {
                    DataType::Int64 => Arc::new(
                        ints(&v)?
                            .iter()
                            .map(|x| x.map(i64::wrapping_neg))
                            .collect::<Int64Array>(),
                    ),
                    _ => Arc::new(
                        floats(&v)?
                            .iter()
                            .map(|x| x.map(|x| -x))
                            .collect::<Float64Array>(),
                    ),
                }
            }
            Expr::Not(e) => Arc::new(not(&bools(&e.eval(cx)?)?)?),
            Expr::Binary(op, a, b) => self.binary(*op, a, b, cx)?,
            Expr::Call(name, args) => self.call(name, args, cx)?,
        })
    }

    fn binary(&self, op: BinOp, a: &Expr, b: &Expr, cx: &Context<'_>) -> Result<ArrayRef> {
        let (va, vb) = (a.eval(cx)?, b.eval(cx)?);
        let (ta, tb) = (va.data_type().clone(), vb.data_type().clone());
        Ok(match op {
            BinOp::Or => Arc::new(or_kleene(&bools(&va)?, &bools(&vb)?)?),
            BinOp::And => Arc::new(and_kleene(&bools(&va)?, &bools(&vb)?)?),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let t = unify(&ta, &tb).ok_or_else(|| mismatch("compare", &ta, &tb))?;
                if t == DataType::Null {
                    return Ok(new_null_array(&DataType::Boolean, cx.len()));
                }
                let (l, r) = (to(&va, &t)?, to(&vb, &t)?);
                Arc::new(match op {
                    BinOp::Eq => cmp::eq(&l, &r)?,
                    BinOp::Ne => cmp::neq(&l, &r)?,
                    BinOp::Lt => cmp::lt(&l, &r)?,
                    BinOp::Le => cmp::lt_eq(&l, &r)?,
                    BinOp::Gt => cmp::gt(&l, &r)?,
                    _ => cmp::gt_eq(&l, &r)?,
                })
            }
            BinOp::Concat => {
                let (l, r) = (strings(&va)?, strings(&vb)?);
                Arc::new(
                    l.iter()
                        .zip(r.iter())
                        .map(|(l, r)| Some(format!("{}{}", l?, r?)))
                        .collect::<StringArray>(),
                )
            }
            BinOp::Div => Arc::new(div(&floats(&va)?, &floats(&vb)?)?),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Rem => {
                let float = ta == DataType::Float64 || tb == DataType::Float64;
                if float {
                    let (l, r) = (floats(&va)?, floats(&vb)?);
                    match op {
                        BinOp::Add => add_wrapping(&l, &r)?,
                        BinOp::Sub => sub_wrapping(&l, &r)?,
                        BinOp::Mul => mul_wrapping(&l, &r)?,
                        _ => Arc::new(
                            l.iter()
                                .zip(r.iter())
                                .map(|(l, r)| Some(l? % r?))
                                .collect::<Float64Array>(),
                        ),
                    }
                } else {
                    let (l, r) = (ints(&va)?, ints(&vb)?);
                    match op {
                        BinOp::Add => add_wrapping(&l, &r)?,
                        BinOp::Sub => sub_wrapping(&l, &r)?,
                        BinOp::Mul => mul_wrapping(&l, &r)?,
                        _ => Arc::new(
                            l.iter()
                                .zip(r.iter())
                                .map(|(l, r)| l?.checked_rem(r?))
                                .collect::<Int64Array>(),
                        ),
                    }
                }
            }
        })
    }

    fn call(&self, name: &str, args: &[Expr], cx: &Context<'_>) -> Result<ArrayRef> {
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

    pub(super) fn output_fields(&self, schema: &Schema) -> Result<Vec<arrow::datatypes::Field>> {
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

    pub(super) fn apply(
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
