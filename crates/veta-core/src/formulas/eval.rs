//! Evaluating an [`Expr`] on a batch of rows with arrow kernels.

use super::*;

// Evaluation.

/// What an expression is evaluated over.
pub(crate) struct Context<'a> {
    pub batch: &'a RecordBatch,
    /// Position of the batch's first row in the step's input.
    pub first_row: usize,
}

impl Context<'_> {
    pub(super) fn len(&self) -> usize {
        self.batch.num_rows()
    }
}

pub(super) fn to(array: &ArrayRef, t: &DataType) -> Result<ArrayRef> {
    if array.data_type() == t {
        Ok(array.clone())
    } else {
        Ok(cast(array, t)?)
    }
}

pub(super) fn floats(array: &ArrayRef) -> Result<Float64Array> {
    let array = to(array, &DataType::Float64)?;
    Ok(array
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| invalid("expected a number".into()))?
        .clone())
}

pub(super) fn ints(array: &ArrayRef) -> Result<Int64Array> {
    let array = to(array, &DataType::Int64)?;
    Ok(array
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| invalid("expected a whole number".into()))?
        .clone())
}

pub(super) fn strings(array: &ArrayRef) -> Result<StringArray> {
    let array = to(array, &DataType::Utf8)?;
    Ok(array
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| invalid("expected text".into()))?
        .clone())
}

pub(super) fn bools(array: &ArrayRef) -> Result<BooleanArray> {
    let array = to(array, &DataType::Boolean)?;
    Ok(array
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or_else(|| invalid("expected true/false".into()))?
        .clone())
}

/// Nulls in a condition count as false.
pub(super) fn condition(array: &ArrayRef) -> Result<BooleanArray> {
    let mask = bools(array)?;
    Ok(if mask.nulls().is_some() {
        prep_null_mask_filter(&mask)
    } else {
        mask
    })
}

pub(super) fn map_text(array: &ArrayRef, f: impl Fn(&str) -> String) -> Result<ArrayRef> {
    let s = strings(array)?;
    Ok(Arc::new(
        s.iter().map(|v| v.map(&f)).collect::<StringArray>(),
    ))
}

impl Expr {
    /// Evaluates the expression; the result has [`Expr::data_type`]'s type.
    pub(crate) fn eval(&self, cx: &Context<'_>) -> Result<ArrayRef> {
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
}
