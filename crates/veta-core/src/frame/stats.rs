//! Statistics: one per [`Column`] method, each computing only what it
//! needs, or several for several columns in one pass ([`DataFrame::stats`],
//! [`DataFrame::describe`]). Exact; on a sample, call them on
//! `df.sample(fraction, seed)?`.

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use arrow::row::{RowConverter, SortField};

use super::{Column, DataFrame};
use crate::error::Result;
use crate::ops::Columns;
use crate::plan::Task;
use crate::stats::{Aggregate, Estimate, Interpolation, Needs, Stat, StatValue};

impl DataFrame {
    /// The running statistics of the columns at `positions`, each with what
    /// its `needs` say, in one pass.
    fn aggregate(&self, positions: &[usize], needs: &[Needs]) -> Result<Vec<Aggregate>> {
        let schema = self.schema();
        let fields: Vec<(String, DataType)> = positions
            .iter()
            .map(|&p| {
                (
                    schema.field(p).name().clone(),
                    schema.field(p).data_type().clone(),
                )
            })
            .collect();
        let new = {
            let (fields, needs) = (fields.clone(), needs.to_vec());
            move || -> Result<Vec<Aggregate>> {
                fields
                    .iter()
                    .zip(&needs)
                    .map(|((name, data_type), needs)| Aggregate::new(*needs, data_type, name))
                    .collect()
            }
        };
        let mut whole = new()?;
        let pieces = self.node.read_parallel(
            Some(positions.to_vec()),
            self.reporting(Task::Computing),
            move |batches, first| {
                let mut piece = new()?;
                let mut row = first as u64;
                for batch in &batches {
                    for (aggregate, column) in piece.iter_mut().zip(batch.columns()) {
                        aggregate.add(column, row)?;
                    }
                    row += batch.num_rows() as u64;
                }
                Ok(piece)
            },
        );
        for piece in pieces {
            for (mine, theirs) in whole.iter_mut().zip(piece?) {
                mine.merge(theirs);
            }
        }
        Ok(whole)
    }

    /// `stats` of each of `columns` (`..` for all), in one pass: one list
    /// per column, in the order given.
    pub fn stats(
        &self,
        columns: impl Into<Columns>,
        stats: &[Stat],
    ) -> Result<Vec<Vec<StatValue>>> {
        let positions = columns.into().positions(self.schema_ref())?;
        let needs = vec![Needs::of(stats); positions.len()];
        let aggregates = self.aggregate(&positions, &needs)?;
        aggregates
            .into_iter()
            .zip(&positions)
            .map(|(mut aggregate, &position)| {
                stats
                    .iter()
                    .map(|&stat| match stat {
                        Stat::Mode => self.mode_value(&aggregate, position),
                        stat => Ok(aggregate.finish(stat)),
                    })
                    .collect()
            })
            .collect()
    }

    /// The mode found by `aggregate`, read from the column.
    fn mode_value(&self, aggregate: &Aggregate, position: usize) -> Result<StatValue> {
        Ok(StatValue::Value(match aggregate.mode_row() {
            Some(row) => self.value(row as usize, position)?,
            None => arrow::array::new_null_array(self.schema_ref().field(position).data_type(), 1),
        }))
    }

    /// A summary of every column, as a frame: a `statistic` column naming
    /// each row (`count`, `null_count`, `n_unique`, `mean`, `std`, `min`,
    /// `25%`, `50%`, `75%`, `max`) and the columns' values as text (empty
    /// where a statistic doesn't apply). In one pass.
    pub fn describe(&self) -> Result<DataFrame> {
        const ROWS: [&str; 10] = [
            "count",
            "null_count",
            "n_unique",
            "mean",
            "std",
            "min",
            "25%",
            "50%",
            "75%",
            "max",
        ];
        let schema = self.schema();
        let positions: Vec<usize> = (0..schema.fields().len()).collect();
        // What applies to each column: counts always, distinct and range
        // when its values compare, the rest for numbers.
        let applies: Vec<(bool, bool)> = schema
            .fields()
            .iter()
            .map(|f| {
                let sortable =
                    RowConverter::supports_fields(&[SortField::new(f.data_type().clone())]);
                let numeric = f.data_type().is_numeric() || f.data_type().is_temporal();
                (sortable, numeric)
            })
            .collect();
        let needs: Vec<Needs> = applies
            .iter()
            .map(|&(sortable, numeric)| {
                let mut stats = vec![Stat::CountValues];
                if sortable {
                    stats.extend([Stat::NUnique, Stat::Min, Stat::Max]);
                }
                if numeric {
                    stats.extend([Stat::Mean, Stat::Median]);
                }
                Needs::of(&stats)
            })
            .collect();
        let aggregates = self.aggregate(&positions, &needs)?;
        let mut columns: Vec<ArrayRef> = vec![Arc::new(StringArray::from(ROWS.to_vec()))];
        let mut fields = vec![Field::new("statistic", DataType::Utf8, false)];
        for ((mut aggregate, &(sortable, numeric)), field) in
            aggregates.into_iter().zip(&applies).zip(schema.fields())
        {
            let count = |a: &mut Aggregate, stat| a.finish(stat).as_count().map(|n| n.to_string());
            let number = |x: Option<f64>| {
                x.and_then(|x| {
                    text(&(Arc::new(arrow::array::Float64Array::from(vec![x])) as ArrayRef))
                })
            };
            let value = |a: &mut Aggregate, stat| match a.finish(stat) {
                StatValue::Value(v) => text(&v),
                _ => None,
            };
            let quantile = |a: &mut Aggregate, q| number(a.quantile(q, Interpolation::Linear));
            let cells: Vec<Option<String>> = vec![
                count(&mut aggregate, Stat::CountValues),
                count(&mut aggregate, Stat::CountNulls),
                sortable
                    .then(|| count(&mut aggregate, Stat::NUnique))
                    .flatten(),
                numeric
                    .then(|| number(aggregate.finish(Stat::Mean).as_number()))
                    .flatten(),
                numeric
                    .then(|| number(aggregate.variance(Estimate::Population).map(f64::sqrt)))
                    .flatten(),
                sortable.then(|| value(&mut aggregate, Stat::Min)).flatten(),
                numeric.then(|| quantile(&mut aggregate, 0.25)).flatten(),
                numeric.then(|| quantile(&mut aggregate, 0.5)).flatten(),
                numeric.then(|| quantile(&mut aggregate, 0.75)).flatten(),
                sortable.then(|| value(&mut aggregate, Stat::Max)).flatten(),
            ];
            columns.push(Arc::new(StringArray::from(cells)));
            fields.push(Field::new(field.name(), DataType::Utf8, true));
        }
        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(schema.clone(), columns)?;
        DataFrame::from_batches(schema, vec![batch])
    }
}

/// One value as text, `None` when null.
fn text(value: &ArrayRef) -> Option<String> {
    if value.is_null(0) {
        return None;
    }
    let text = arrow::compute::cast(value, &DataType::Utf8).ok()?;
    text.as_any()
        .downcast_ref::<StringArray>()
        .map(|s| s.value(0).to_owned())
}

impl Column {
    /// The running statistics of this column for `stats`.
    fn aggregate(&self, stats: &[Stat]) -> Result<Aggregate> {
        let mut all = self
            .frame
            .aggregate(&[self.position], &[Needs::of(stats)])?;
        Ok(all.remove(0))
    }

    fn one(&self, stat: Stat) -> Result<StatValue> {
        Ok(self.aggregate(&[stat])?.finish(stat))
    }

    fn count(&self, stat: Stat) -> Result<u64> {
        Ok(self.one(stat)?.as_count().unwrap_or(0))
    }

    fn number(&self, stat: Stat) -> Result<Option<f64>> {
        Ok(self.one(stat)?.as_number())
    }

    fn value_of(&self, stat: Stat) -> Result<ArrayRef> {
        match self.one(stat)? {
            StatValue::Value(value) => Ok(value),
            _ => Ok(arrow::array::new_null_array(self.data_type(), 1)),
        }
    }

    /// Values that aren't null.
    pub fn count_values(&self) -> Result<u64> {
        self.count(Stat::CountValues)
    }

    pub fn count_nulls(&self) -> Result<u64> {
        self.count(Stat::CountNulls)
    }

    /// Distinct values, nulls left out.
    pub fn n_unique(&self) -> Result<u64> {
        self.count(Stat::NUnique)
    }

    /// The smallest value (an array of one; null when there are none).
    pub fn min(&self) -> Result<ArrayRef> {
        self.value_of(Stat::Min)
    }

    /// The largest value (an array of one; null when there are none).
    pub fn max(&self) -> Result<ArrayRef> {
        self.value_of(Stat::Max)
    }

    /// The sum of the numbers (0 when there are none).
    pub fn sum(&self) -> Result<f64> {
        Ok(self.number(Stat::Sum)?.unwrap_or(0.0))
    }

    /// The mean (dates and times: of their underlying numbers).
    pub fn mean(&self) -> Result<Option<f64>> {
        self.number(Stat::Mean)
    }

    pub fn median(&self) -> Result<Option<f64>> {
        self.number(Stat::Median)
    }

    /// The standard deviation of the values as a population (dividing by
    /// n).
    pub fn std(&self) -> Result<Option<f64>> {
        self.std_with(Estimate::Population)
    }

    /// The standard deviation, as a population or as a sample (dividing by
    /// n − 1).
    pub fn std_with(&self, estimate: Estimate) -> Result<Option<f64>> {
        Ok(self.var_with(estimate)?.map(f64::sqrt))
    }

    /// The variance of the values as a population (dividing by n).
    pub fn var(&self) -> Result<Option<f64>> {
        self.var_with(Estimate::Population)
    }

    /// The variance, as a population or as a sample (dividing by n − 1).
    pub fn var_with(&self, estimate: Estimate) -> Result<Option<f64>> {
        Ok(self.aggregate(&[Stat::Var(estimate)])?.variance(estimate))
    }

    /// The `q` quantile (0 to 1), interpolating linearly.
    pub fn quantile(&self, q: f64) -> Result<Option<f64>> {
        self.quantile_with(q, Interpolation::Linear)
    }

    /// The `q` quantile, taken as `how` says.
    pub fn quantile_with(&self, q: f64, how: Interpolation) -> Result<Option<f64>> {
        Ok(self.aggregate(&[Stat::Quantile(q)])?.quantile(q, how))
    }

    /// The most frequent value (the first to appear, of equals; an array of
    /// one, null when there are none).
    pub fn mode(&self) -> Result<ArrayRef> {
        let aggregate = self.aggregate(&[Stat::Mode])?;
        match self.frame.mode_value(&aggregate, self.position)? {
            StatValue::Value(value) => Ok(value),
            _ => Ok(arrow::array::new_null_array(self.data_type(), 1)),
        }
    }

    /// The skewness of the values as a population (m₃/m₂^1.5).
    pub fn skew(&self) -> Result<Option<f64>> {
        self.skew_with(Estimate::Population)
    }

    /// The skewness, as a population or as a sample (bias-corrected).
    pub fn skew_with(&self, estimate: Estimate) -> Result<Option<f64>> {
        self.number(Stat::Skew(estimate))
    }

    /// The excess kurtosis of the values as a population (m₄/m₂² − 3).
    pub fn kurtosis(&self) -> Result<Option<f64>> {
        self.kurtosis_with(Estimate::Population)
    }

    /// The excess kurtosis, as a population or as a sample
    /// (bias-corrected).
    pub fn kurtosis_with(&self, estimate: Estimate) -> Result<Option<f64>> {
        self.number(Stat::Kurtosis(estimate))
    }
}
