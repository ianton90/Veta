//! The Steps tab: applied steps.

use super::*;
use iced::widget::column;

/// The applied steps list. Selecting a step shows the data after it.
pub(super) fn steps_pane<'a>(tab: &'a Tab, doc: &'a Document, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
    let steps = doc.steps();
    let evaluated = doc.evaluated_steps();
    let shown = tab.grid.preview().unwrap_or(evaluated).min(evaluated);
    let red = Color::from_rgb8(0xd1, 0x43, 0x43);

    let item = |index: Option<usize>, label: String, note: Option<(String, Color)>| {
        // `index` is the step position; None is the source.
        let level = index.map_or(0, |i| i + 1);
        let selected = level == shown;
        let available = level <= evaluated;
        let color = if available { t.text } else { t.muted_text };
        let mut text_col = column![text(label).size(ui.small()).color(color)].spacing(2);
        if let Some((note, note_color)) = note {
            text_col = text_col.push(text(note).size(ui.small() - 1.0).color(note_color));
        }
        let mut select = button(text_col.width(Length::Fill))
            .width(Length::Fill)
            .padding([5, 8])
            .style(move |_theme: &Theme, status| {
                let mut style = subtle_button(t, status);
                if selected {
                    style.background = Some(Background::Color(Color {
                        a: 0.14,
                        ..t.selection
                    }));
                }
                style
            });
        if available {
            select = select.on_press(Message::PreviewStep(tab.id, level));
        }
        let mut line = row![select].spacing(2).align_y(Alignment::Center);
        if let Some(i) = index {
            let small = |glyph: Icon, message: Option<Message>, tip: &'static str| {
                let color = if message.is_some() {
                    t.muted_text
                } else {
                    Color { a: 0.2, ..t.text }
                };
                let mut b = button(icon(glyph, ui.small()).color(color))
                    .padding([4, 5])
                    .style(move |_theme: &Theme, status| subtle_button(t, status));
                if let Some(message) = message {
                    b = b.on_press(message);
                }
                iced::widget::tooltip(
                    b,
                    text(tip).size(ui.small() - 1.0),
                    iced::widget::tooltip::Position::Top,
                )
            };
            let editable = crate::step_dialog_supported(&steps[i]);
            line = line
                .push(small(
                    Icon::ChevronUp,
                    i.checked_sub(1).map(|up| Message::MoveStep(i, up)),
                    "Move up",
                ))
                .push(small(
                    Icon::ChevronDown,
                    (i + 1 < steps.len()).then_some(Message::MoveStep(i, i + 1)),
                    "Move down",
                ))
                .push(small(
                    Icon::Rename,
                    editable.then_some(Message::EditStep(i)),
                    "Edit",
                ))
                .push(small(Icon::Trash, Some(Message::RemoveStep(i)), "Remove"));
        }
        line.into()
    };

    let mut list: Vec<Element<'a, Message>> = vec![item(None, "Source".into(), None)];
    for (i, step) in steps.iter().enumerate() {
        let note = match doc.status() {
            Status::Broken { step, error } if *step == i => Some((error.clone(), red)),
            Status::Broken { step, .. } if i > *step => {
                Some(("Not evaluated".into(), t.muted_text))
            }
            Status::Computing { step } if *step == i => Some(("Computing…".into(), t.primary)),
            Status::Computing { step } if i > *step => Some(("Waiting".into(), t.muted_text)),
            _ => None,
        };
        list.push(item(
            Some(i),
            format!("{}. {}", i + 1, step.describe()),
            note,
        ));
    }
    let mut content = column(list).spacing(2).padding([0, 8]);
    if steps.is_empty() {
        content = content.push(
            text("Changes you make appear here as steps.")
                .size(ui.small())
                .color(t.muted_text),
        );
    }
    let footer_button = |label: &'static str, message: Option<Message>| {
        let mut b = button(text(label).size(ui.small()))
            .padding([4, 8])
            .style(move |_theme: &Theme, status| subtle_button(t, status));
        if let Some(message) = message {
            b = b.on_press(message);
        }
        b
    };
    let footer = row![
        footer_button(
            "Save recipe…",
            (!steps.is_empty()).then_some(Message::SaveRecipe)
        ),
        footer_button("Load recipe…", Some(Message::LoadRecipe)),
    ]
    .spacing(4)
    .padding([4, 8]);
    column![scrollable(content).height(Length::Fill), footer].into()
}
