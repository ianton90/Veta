//! The status bar under the grid.

use super::*;

pub fn status_bar(
    active: Option<(&Tab, &Document)>,
    opening: usize,
    transfers: &[crate::Transfer],
    notice: Option<&str>,
    ui: Ui,
) -> Element<'static, Message> {
    let mut parts: Vec<String> = Vec::new();
    if let Some((tab, doc)) = active {
        parts.push(format!("{} rows", doc.num_rows()));
        parts.push(format!("{} columns", doc.num_columns()));
        parts.push(
            match doc.source_mode() {
                SourceMode::InMemory => "In memory",
                SourceMode::Paged => "Paged",
            }
            .into(),
        );
        if let Some((r, c)) = tab.grid.selected() {
            let name = doc
                .schema()
                .fields()
                .get(c)
                .map(|f| format!(", {}", f.name()))
                .unwrap_or_default();
            parts.push(format!("Row {}{name}", r + 1));
        }
        parts.push(
            if doc.is_modified() {
                "Unsaved changes"
            } else {
                "No changes"
            }
            .into(),
        );
        match doc.status() {
            Status::Broken { step, .. } => parts.push(format!("Step {} has an error", step + 1)),
            Status::Computing { step } => parts.push(format!("Computing step {}…", step + 1)),
            Status::Ready => {}
        }
        if let Some(e) = tab.grid.error() {
            parts.push(format!("Read error: {e}"));
        }
    }
    if opening > 0 {
        parts.push(format!("Opening {opening} file(s)…"));
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
            button(text("Cancel").size(ui.small()))
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
