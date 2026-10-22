//! The status bar under the grid.

use super::*;

pub fn status_bar(
    active: Option<(&Tab, &Document)>,
    opening: usize,
    transfers: &[crate::app::Transfer],
    notice: Option<&str>,
    zoom: u16,
    ui: Ui,
) -> Element<'static, Message> {
    let mut parts: Vec<String> = Vec::new();
    if let Some((tab, doc)) = active {
        parts.push(tr_with("ST_STATUS_ROWS", &[("n", &doc.num_rows())]));
        parts.push(tr_with("ST_STATUS_COLUMNS", &[("n", &doc.num_columns())]));
        parts.push(tr(match doc.source_mode() {
            SourceMode::InMemory => "ST_STATUS_IN_MEMORY",
            SourceMode::Paged => "ST_STATUS_PAGED",
        }));
        if let Some((r, c)) = tab.grid.selected() {
            let name = doc
                .schema()
                .fields()
                .get(c)
                .map(|f| format!(", {}", f.name()))
                .unwrap_or_default();
            parts.push(format!(
                "{}{name}",
                tr_with("ST_STATUS_ROW", &[("n", &(r + 1))])
            ));
        }
        parts.push(tr(if doc.is_modified() {
            "ST_STATUS_UNSAVED"
        } else {
            "ST_STATUS_NO_CHANGES"
        }));
        match doc.status() {
            Status::Broken { step, .. } => {
                parts.push(tr_with("ST_STATUS_STEP_ERROR", &[("n", &(step + 1))]));
            }
            Status::Computing { step } => {
                parts.push(tr_with("ST_STATUS_COMPUTING_STEP", &[("n", &(step + 1))]));
            }
            Status::Ready => {}
        }
        if let Some(e) = tab.grid.error() {
            parts.push(tr_with("ST_STATUS_READ_ERROR", &[("error", &e)]));
        }
    }
    if zoom != 100 {
        parts.push(tr_with("ST_STATUS_ZOOM", &[("n", &zoom)]));
    }
    if opening > 0 {
        parts.push(tr_with("ST_STATUS_OPENING", &[("n", &opening)]));
    }
    if transfers.is_empty()
        && let Some(notice) = notice
    {
        parts.push(notice.to_owned());
    }
    let t = ui.tokens;
    let mut bar = row![
        text(parts.join("   ·   "))
            .size(ui.small())
            .color(t.muted_text)
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    for transfer in transfers {
        bar = bar.push(
            text(format!(
                "{} {:.0}%",
                transfer.label,
                transfer.progress * 100.0
            ))
            .size(ui.small())
            .color(t.text),
        );
        bar = bar.push(
            button(text(tr("ST_COMMON_CANCEL")).size(ui.small()))
                .padding([0, 8])
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::CancelTransfer(transfer.id)),
        );
    }
    container(bar)
        .padding([4, 12])
        .width(Length::Fill)
        .style(move |_theme: &Theme| surface(t))
        .into()
}
