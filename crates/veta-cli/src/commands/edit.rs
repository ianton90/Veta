//! Editing subcommands: each opens the file, runs core commands and saves, in place or to `--output`.

use std::io::Write;
use std::path::PathBuf;

use super::*;
use crate::Error;

/// Which rows: `--top N`, `--bottom N` or `--rows` ranges.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(group(clap::ArgGroup::new("which").required(true).args(["top", "bottom", "rows"])))]
pub struct RowsArgs {
    file: PathBuf,
    /// The first N rows.
    #[arg(long, value_name = "N")]
    top: Option<usize>,
    /// The last N rows.
    #[arg(long, value_name = "N")]
    bottom: Option<usize>,
    /// Row numbers and ranges, from 1, e.g. 5 or 10-20.
    #[arg(long)]
    rows: Vec<String>,
}

impl RowsArgs {
    /// The rows meant, 0-based, in a table of `num_rows` rows.
    fn ranges(&self, num_rows: usize) -> Result<Vec<std::ops::Range<usize>>, Error> {
        use veta_core::rows::{RowPlace, row_range};
        match (self.top, self.bottom) {
            (Some(n), _) => Ok(vec![row_range(RowPlace::Top, n, num_rows)?]),
            (_, Some(n)) => Ok(vec![row_range(RowPlace::Bottom, n, num_rows)?]),
            _ => parse_rows(&self.rows),
        }
    }
}

/// Runs an editing subcommand.
pub(crate) fn run(command: crate::Command, out: &mut impl Write) -> Result<(), Error> {
    use crate::Command as C;
    use veta_core::Command as CoreCommand;
    match command {
        C::SetCell {
            file,
            row,
            column,
            value,
            null,
            output,
        } => {
            let row = row_index(row)?;
            let value = if null { None } else { value };
            edit(&file, &output, out, |_| {
                Ok(vec![CoreCommand::SetCell { row, column, value }])
            })
        }
        C::InsertRows {
            file,
            top,
            bottom,
            at,
            count,
            output,
        } => {
            use veta_core::rows::{RowAction, RowPlace, rows_command};
            let place = match (top, bottom, at) {
                (true, _, _) => RowPlace::Top,
                (_, true, _) => RowPlace::Bottom,
                (_, _, Some(at)) => RowPlace::At(row_index(at)?),
                _ => return Err(Error::Usage("use --top, --bottom or --at".into())),
            };
            edit(&file, &output, out, |doc| {
                Ok(vec![rows_command(
                    RowAction::Insert,
                    place,
                    count,
                    doc.num_rows(),
                )?])
            })
        }
        C::DeleteRows { rows, output } => edit(&rows.file, &output, out, |doc| {
            let rows = rows.ranges(doc.num_rows())?;
            Ok(vec![CoreCommand::DeleteRows { rows }])
        }),
        C::KeepRows { rows, output } => edit(&rows.file, &output, out, |doc| {
            let keep = rows.ranges(doc.num_rows())?;
            if let Some(bad) = keep.iter().find(|r| r.end > doc.num_rows()) {
                return Err(Error::Usage(format!(
                    "row {} is past the last row ({})",
                    bad.end,
                    doc.num_rows()
                )));
            }
            let delete = veta_core::rows::keep_only(&keep, doc.num_rows());
            Ok(if delete.is_empty() {
                Vec::new()
            } else {
                vec![CoreCommand::DeleteRows { rows: delete }]
            })
        }),
        C::AddColumn {
            file,
            name,
            data_type,
            at,
            output,
        } => {
            let data_type = parse_type(&data_type)?;
            let at = at.map(row_index).transpose()?;
            edit(&file, &output, out, |doc| {
                Ok(vec![CoreCommand::AddColumn {
                    name,
                    data_type,
                    at: at.unwrap_or(doc.num_columns()),
                }])
            })
        }
        C::RemoveColumn {
            file,
            names,
            output,
        } => edit(&file, &output, out, |_| {
            Ok(vec![CoreCommand::RemoveColumns { names }])
        }),
        C::RenameColumn {
            file,
            from,
            to,
            output,
        } => edit(&file, &output, out, |_| {
            Ok(vec![CoreCommand::RenameColumn { from, to }])
        }),
        C::MoveColumn {
            file,
            name,
            to,
            output,
        } => {
            let to = row_index(to)?;
            edit(&file, &output, out, |_| {
                Ok(vec![CoreCommand::MoveColumn { name, to }])
            })
        }
        _ => Err(Error::Usage("not an editing command".into())),
    }
}
