//! The Statistics tab.

use super::*;
use iced::widget::column;

/// What the statistics tab shows.
#[derive(Debug, Default)]
pub struct StatsView {
    /// Per selected column: its statistics so far, and whether they are only
    /// the quick ones from the file's footer (or none yet).
    pub columns: Vec<(String, Option<ColumnStats>, bool)>,
    /// Progress of the pass computing the missing ones.
    pub progress: Option<f32>,
    /// Why statistics can't be computed right now.
    pub waiting: Option<String>,
}

/// Statistics of the selected columns.
pub(super) fn stats_pane<'a>(view: StatsView, ui: Ui) -> Element<'a, Message> {
    use veta_fmt::{format_distinct, format_float, format_stat_value};
    let t = ui.tokens;
    let mut content = column![].spacing(10).padding([0, 12]);
    if let Some(reason) = view.waiting {
        content = content.push(text(reason).size(ui.small()).color(t.muted_text));
    }
    if view.columns.is_empty() {
        content = content.push(
            text(tr("ST_STATS_SELECT"))
                .size(ui.small())
                .color(t.muted_text),
        );
    }
    if let Some(progress) = view.progress {
        content = content.push(
            column![
                text(format!(
                    "{} {:.0}%",
                    tr("ST_PANE_COMPUTING"),
                    progress * 100.0
                ))
                .size(ui.small())
                .color(t.muted_text),
                progress_bar(0.0..=1.0, progress).girth(4),
            ]
            .spacing(4),
        );
    }
    let pending = "…".to_owned();
    for (name, stats, partial) in view.columns {
        let mut card = column![
            row![
                text(name).size(ui.size).font(bold()).width(Length::Fill),
                text(
                    stats
                        .as_ref()
                        .map(|s| s.type_name.clone())
                        .unwrap_or_default()
                )
                .size(ui.small())
                .color(t.muted_text),
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(3);
        let line = |label: &str, value: Option<String>| {
            row![
                text(tr(label))
                    .size(ui.small())
                    .color(t.muted_text)
                    .width(90),
                text(value.unwrap_or_else(|| "—".into())).size(ui.small()),
            ]
        };
        let (count, nulls, distinct, min, max, mean, sd) = match &stats {
            Some(s) => {
                let percent = if s.count > 0 {
                    format!(" ({:.1}%)", s.nulls as f64 * 100.0 / s.count as f64)
                } else {
                    String::new()
                };
                let later = |v: Option<String>| if partial { Some(pending.clone()) } else { v };
                (
                    Some(s.count.to_string()),
                    Some(format!("{}{percent}", s.nulls)),
                    later(format_distinct(s)),
                    s.min.as_ref().map(format_stat_value),
                    s.max.as_ref().map(format_stat_value),
                    later(s.mean.as_ref().map(format_stat_value)),
                    later(s.std_dev.as_ref().map(format_stat_value)),
                )
            }
            None => {
                let p = Some(pending.clone());
                (
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p.clone(),
                    p,
                )
            }
        };
        card = card
            .push(line("ST_STATS_COUNT", count))
            .push(line("ST_STATS_NULLS", nulls))
            .push(line("ST_STATS_DISTINCT", distinct))
            .push(line("ST_STATS_MIN", min))
            .push(line("ST_STATS_MAX", max))
            .push(line("ST_STATS_MEAN", mean))
            .push(line("ST_STATS_STD_DEV", sd));
        if let Some(l) = stats.as_ref().and_then(|s| s.text_lengths.as_ref()) {
            let value = tr_with(
                "ST_STATS_LENGTH_VALUE",
                &[
                    ("min", &l.min),
                    ("max", &l.max),
                    ("mean", &format_float(l.mean)),
                ],
            );
            card = card.push(line("ST_STATS_LENGTH", (!partial).then_some(value)));
        }
        content = content.push(container(card).padding(8).width(Length::Fill).style(
            move |_theme: &Theme| {
                container::Style::default()
                    .background(t.surface)
                    .border(Border {
                        color: t.border,
                        width: 1.0,
                        radius: 6.0.into(),
                    })
            },
        ));
    }
    scrollable(content).height(Length::Fill).into()
}
