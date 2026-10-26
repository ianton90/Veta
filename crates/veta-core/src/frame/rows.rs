//! Row methods: sort, reverse, sample, insert, drop nulls, unique, buffer,
//! implode.

use super::DataFrame;
use crate::error::Result;
use crate::ops::{
    Buffer, Columns, DropDuplicates, DropNulls, Implode, InsertRows, Keep, Reverse, Sample,
    SortRows,
};
use crate::transforms::sort::{Sort, SortKey};

/// Ascending or descending, for [`DataFrame::sort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

/// How [`DataFrame::sort_with`] sorts.
#[derive(Debug, Clone, Copy, Default)]
pub struct SortOptions {
    /// Nulls before other values (otherwise after).
    pub nulls_first: bool,
}

impl DataFrame {
    /// Sorts rows by `keys`, the first first (stable):
    /// `df.sort([("amount", Desc), ("name", Asc)])`.
    pub fn sort<K: Into<String>>(
        &self,
        keys: impl IntoIterator<Item = (K, Order)>,
    ) -> Result<Self> {
        self.sort_with(keys, SortOptions::default())
    }

    /// Like [`Self::sort`], with options.
    pub fn sort_with<K: Into<String>>(
        &self,
        keys: impl IntoIterator<Item = (K, Order)>,
        options: SortOptions,
    ) -> Result<Self> {
        let keys = keys
            .into_iter()
            .map(|(column, order)| SortKey {
                column: column.into(),
                descending: order == Order::Desc,
                nulls_first: options.nulls_first,
            })
            .collect();
        self.then(SortRows(Sort { keys }))
    }

    /// The rows in reverse order.
    pub fn reverse(&self) -> Result<Self> {
        self.then(Reverse)
    }

    /// About `fraction` (0 to 1) of the rows, picked by their number and
    /// `seed`: the same seed picks the same rows, for every column.
    pub fn sample(&self, fraction: f64, seed: u64) -> Result<Self> {
        self.then(Sample { fraction, seed })
    }

    /// `count` empty rows at row `at` (columns allow nulls from then on).
    pub fn insert_rows(&self, at: usize, count: usize) -> Result<Self> {
        self.then(InsertRows { at, count })
    }

    /// Drops the rows with a null in any of `columns`: `df.drop_nulls(..)`
    /// for any column.
    pub fn drop_nulls(&self, columns: impl Into<Columns>) -> Result<Self> {
        self.then(DropNulls(columns.into()))
    }

    /// One row of each group of rows equal in `columns` (`..` for all), in
    /// their order: the first, the last, or none of a group of several.
    pub fn unique(&self, columns: impl Into<Columns>, keep: Keep) -> Result<Self> {
        self.then(DropDuplicates {
            columns: columns.into(),
            keep,
        })
    }

    /// Keeps the output so far whole (in memory, or in a temp file past the
    /// memory budget): later steps read from it instead of running the
    /// steps before again. Recomputed when one of them changes.
    pub fn buffer(&self) -> Result<Self> {
        self.then(Buffer)
    }

    /// One row per group of equal `keys` (sorted by them), with `columns`
    /// collected into lists; other columns are dropped.
    pub fn implode(&self, keys: impl Into<Columns>, columns: impl Into<Columns>) -> Result<Self> {
        let names = |columns: Columns| -> Result<Vec<String>> {
            let positions = columns.positions(self.schema_ref())?;
            Ok(positions
                .iter()
                .map(|&i| self.schema_ref().field(i).name().clone())
                .collect())
        };
        self.then(Implode {
            keys: names(keys.into())?,
            columns: names(columns.into())?,
        })
    }
}
