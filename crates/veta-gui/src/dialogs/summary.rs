//! Statistics of every column at once, as a table.

use super::*;
use iced::widget::{column, table};
use veta_core::stats::ColumnStats;
use veta_fmt::{format_distinct, format_median, format_stat_value, format_top_value};

/// One column's row: its name, its statistics so far, and whether they are
/// only the quick ones (or none yet).
pub type SummaryRow = (String, Option<ColumnStats>, bool);

/// The summary of `rows`, with the progress of the pass computing them.
pub fn summary<'a, Message: Clone + 'a>(
    rows: Vec<SummaryRow>,
    progress: Option<f32>,
    close: Message,
    tokens: Tokens,
) -> Element<'a, Message> {
    let cell = |value: Option<String>| -> Element<'a, Message> {
        text(value.unwrap_or_else(|| "—".into())).size(13).into()
    };
    let header = |tag: &str| -> Element<'a, Message> {
        text(tr(tag)).size(13).font(crate::fonts::bold()).into()
    };
    // A statistic, "…" until the full pass has it.
    let full = |f: fn(&ColumnStats) -> Option<String>| {
        move |(_, stats, partial): SummaryRow| match (&stats, partial) {
            (Some(s), false) => cell(f(s)),
            (Some(_), true) | (None, _) => cell(Some("…".into())),
        }
    };
    // A statistic the file's footer may already give.
    let quick = |f: fn(&ColumnStats) -> Option<String>| {
        move |(_, stats, _): SummaryRow| match &stats {
            Some(s) => cell(f(s)),
            None => cell(Some("…".into())),
        }
    };
    let columns = [
        table::column(header("ST_STATS_COLUMN"), |(name, _, _): SummaryRow| {
            text(name).size(13).font(crate::fonts::bold())
        }),
        table::column(header("ST_STATS_TYPE"), |(_, stats, _): SummaryRow| {
            cell(stats.map(|s| s.type_name))
        }),
        table::column(
            header("ST_STATS_COUNT"),
            quick(|s| Some(s.count.to_string())),
        ),
        table::column(
            header("ST_STATS_NULLS"),
            quick(|s| Some(s.nulls.to_string())),
        ),
        table::column(header("ST_STATS_DISTINCT"), full(format_distinct)),
        table::column(
            header("ST_STATS_MIN"),
            quick(|s| s.min.as_ref().map(format_stat_value)),
        ),
        table::column(
            header("ST_STATS_MAX"),
            quick(|s| s.max.as_ref().map(format_stat_value)),
        ),
        table::column(
            header("ST_STATS_MEAN"),
            full(|s| s.mean.as_ref().map(format_stat_value)),
        ),
        table::column(header("ST_STATS_MEDIAN"), full(format_median)),
        table::column(
            header("ST_STATS_TOP"),
            full(|s| {
                s.top_values
                    .first()
                    .map(|top| format_top_value(top, s.top_estimated, 24))
            }),
        ),
    ];
    let mut body = column![text(tr("ST_STATS_SUMMARY_TITLE")).size(20)].spacing(12);
    if let Some(progress) = progress {
        body = body.push(
            row![
                text(format!(
                    "{} {:.0}%",
                    tr("ST_PANE_COMPUTING"),
                    progress * 100.0
                ))
                .size(13)
                .color(tokens.muted_text),
                iced::widget::progress_bar(0.0..=1.0, progress).girth(4),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    let grid = table(columns, rows).padding_x(10).padding_y(4).separator(1);
    body = body
        .push(
            container(scrollable(grid).direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::default(),
                horizontal: scrollable::Scrollbar::default(),
            }))
            .max_height(520.0),
        )
        .push(row![
            Space::new().width(Length::Fill),
            button(text(tr("ST_COMMON_CLOSE"))).on_press(close),
        ]);
    card(body.into(), tokens, 980.0)
}
