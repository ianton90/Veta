//! The formula bar above the grid.

use super::*;

/// Shows and edits the selected cell, like Excel's formula bar.
pub fn formula_bar<'a>(
    tab: &'a Tab,
    doc: &'a Document,
    edit: Option<&'a Edit>,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    if let Some(at) = tab.grid.preview() {
        let total = doc.steps().len();
        let showing = if at == 0 {
            tr("ST_FORMULA_PREVIEW_SOURCE")
        } else {
            tr_with("ST_FORMULA_PREVIEW_STEP", &[("n", &at), ("total", &total)])
        };
        return container(
            row![
                icon(Icon::Info, ui.size).color(t.primary),
                text(showing).size(ui.small()),
                button(text(tr("ST_FORMULA_SHOW_ALL")).size(ui.small()))
                    .padding([3, 10])
                    .on_press(Message::PreviewStep(tab.id, usize::MAX)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .width(Length::Fill)
        .style(move |_theme: &Theme| {
            container::Style::default().background(Color {
                a: 0.1,
                ..t.primary
            })
        })
        .into();
    }
    let selected = tab.grid.selected();
    let label = match selected {
        Some((row, column)) => {
            let name = doc
                .schema()
                .fields()
                .get(column)
                .map(|f| {
                    let path = tab.grid.selected_field().map_or(&[][..], |(p, _)| p);
                    let mut name = f.name().clone();
                    for part in path {
                        name = format!("{name}.{part}");
                    }
                    if let Some(item) = tab.grid.selected_item() {
                        let key = edit.is_some_and(|e| e.key) && !item.is_slot();
                        name = format!("{name}{}", item.display);
                        if key {
                            name = format!("{name} {}", tr("ST_FORMULA_KEY"));
                        }
                    }
                    format!("{name} · ")
                })
                .unwrap_or_default();
            format!("{name}{}", tr_with("ST_FORMULA_ROW", &[("n", &(row + 1))]))
        }
        None => String::new(),
    };
    let current = tab.grid.selected_edit_value();
    let (value, placeholder) = match (edit, current) {
        (Some(edit), _) => (edit.text.as_str(), ""),
        (None, Some(Some(value))) => (value, ""),
        (None, Some(None)) => ("", "null"),
        (None, None) => ("", ""),
    };
    let mut input = text_input(placeholder, value)
        .id(FORMULA_ID)
        .size(ui.small())
        .padding([3, 6]);
    // Structs, lists and maps open in the grid; they aren't typed as text.
    if selected.is_some() && (edit.is_some() || !tab.grid.selected_is_container()) {
        input = input
            .on_input(Message::EditInput)
            .on_submit(Message::CommitEdit);
    }
    let (hint, hint_color) = match edit {
        Some(Edit {
            error: Some(error), ..
        }) => (error.clone(), t.danger),
        Some(_) => (tr("ST_FORMULA_EDIT_HINT"), t.muted_text),
        None => (String::new(), t.muted_text),
    };
    container(
        row![
            text(label)
                .size(ui.small())
                .width(280)
                .color(t.muted_text)
                .wrapping(text::Wrapping::None),
            icon(Icon::Function, ui.size).color(t.muted_text),
            input,
            text(hint).size(ui.small()).color(hint_color),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .width(Length::Fill)
    .style(move |_theme: &Theme| container::Style::default().background(t.background))
    .into()
}
