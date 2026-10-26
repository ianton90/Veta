//! Values given to [`DataFrame::set`](super::DataFrame::set).

use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, NullArray, StringArray};

/// No value.
#[derive(Debug, Clone, Copy)]
pub struct Null;

/// A single value: a number, text, a boolean, [`Null`], `None`, or an Arrow
/// array of one value. It is converted to the column's type when set.
pub trait IntoValue {
    /// The value as an array of one.
    fn into_value(self) -> ArrayRef;
}

impl IntoValue for Null {
    fn into_value(self) -> ArrayRef {
        Arc::new(NullArray::new(1))
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> ArrayRef {
        Arc::new(Int64Array::from(vec![self]))
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> ArrayRef {
        i64::from(self).into_value()
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> ArrayRef {
        Arc::new(Float64Array::from(vec![self]))
    }
}

impl IntoValue for bool {
    fn into_value(self) -> ArrayRef {
        Arc::new(BooleanArray::from(vec![self]))
    }
}

impl IntoValue for &str {
    fn into_value(self) -> ArrayRef {
        Arc::new(StringArray::from(vec![self]))
    }
}

impl IntoValue for String {
    fn into_value(self) -> ArrayRef {
        self.as_str().into_value()
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> ArrayRef {
        match self {
            Some(value) => value.into_value(),
            None => Null.into_value(),
        }
    }
}

impl IntoValue for ArrayRef {
    fn into_value(self) -> ArrayRef {
        self
    }
}
