//! The Steps tab: applied steps.

use super::*;
use iced::widget::column;
use veta_core::steps::{CellEdits, Step};

/// Edits listed under an expanded edit-cells step; the rest are counted.
const LISTED_EDITS: usize = 200;
/// Width of the arrow that opens an edit-cells step.
const TOGGLE_WIDTH: f32 = 18.0;

/// The applied steps list. Selecting a step shows the data after it.
pub(super) fn steps_pane<'a>(tab: &'a Tab, doc: &'a Document, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
    let steps = doc.steps();
    let evaluated = doc.evaluated_steps();
    let shown = tab.grid.preview().unwrap_or(evaluated).min(evaluated);
    let red = Color::from_rgb8(0xd1, 0x43, 0x43);

    let item = |index: Option<usize>,
                label: String,
                note: Option<(String, Color)>,
                expanded: Option<bool>| {
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
        // Edit-cells steps open to list their edits; other lines keep the
        // arrow's room so labels line up.
        let toggle: Element<'a, Message> = match (index, expanded) {
            (Some(i), Some(open)) => {
                let glyph = if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                };
                button(icon(glyph, ui.small()).color(t.muted_text))
                    .width(TOGGLE_WIDTH)
                    .padding([4, 2])
                    .style(move |_theme: &Theme, status| subtle_button(t, status))
                    .on_press(Message::ToggleStep(tab.id, i))
                    .into()
            }
            _ => iced::widget::Space::new().width(TOGGLE_WIDTH).into(),
        };
        let mut line = row![toggle, select].spacing(2).align_y(Alignment::Center);
        if let Some(i) = index {
            let small = |glyph: Icon, message: Option<Message>, tip: &str| {
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
                    text(tr(tip)).size(ui.small() - 1.0),
                    iced::widget::tooltip::Position::Top,
                )
            };
            let editable = crate::app::step_dialog_supported(&steps[i]);
            line = line
                .push(small(
                    Icon::ChevronUp,
                    i.checked_sub(1).map(|up| Message::MoveStep(i, up)),
                    "ST_STEPS_MOVE_UP",
                ))
                .push(small(
                    Icon::ChevronDown,
                    (i + 1 < steps.len()).then_some(Message::MoveStep(i, i + 1)),
                    "ST_STEPS_MOVE_DOWN",
                ))
                .push(small(
                    Icon::Rename,
                    editable.then_some(Message::EditStep(i)),
                    "ST_STEPS_EDIT",
                ))
                .push(small(
                    Icon::Trash,
                    Some(Message::RemoveStep(i)),
                    "ST_STEPS_REMOVE",
                ));
        }
        line.into()
    };

    let mut list: Vec<Element<'a, Message>> = vec![item(None, tr("ST_STEPS_SOURCE"), None, None)];
    for (i, step) in steps.iter().enumerate() {
        let note = match doc.status() {
            Status::Broken { step, error } if *step == i => Some((error.clone(), red)),
            Status::Broken { step, .. } if i > *step => {
                Some((tr("ST_STEPS_NOT_EVALUATED"), t.muted_text))
            }
            Status::Computing { step } if *step == i => Some((tr("ST_PANE_COMPUTING"), t.primary)),
            Status::Computing { step } if i > *step => Some((tr("ST_STEPS_WAITING"), t.muted_text)),
            _ => None,
        };
        let edits = match step {
            Step::EditCells(edits) => Some(edits),
            _ => None,
        };
        let open = edits.is_some() && tab.expanded_steps.contains(&i);
        list.push(item(
            Some(i),
            format!("{}. {}", i + 1, crate::i18n::model::step_name(step)),
            note,
            edits.map(|_| open),
        ));
        if let Some(edits) = edits.filter(|_| open) {
            list.push(edit_lines(edits, ui));
        }
    }
    let mut content = column(list).spacing(2).padding([0, 8]);
    if steps.is_empty() {
        content = content.push(
            text(tr("ST_STEPS_EMPTY"))
                .size(ui.small())
                .color(t.muted_text),
        );
    }
    let footer_button = |label: &str, message: Option<Message>| {
        let mut b = button(text(tr(label)).size(ui.small()))
            .padding([4, 8])
            .style(move |_theme: &Theme, status| subtle_button(t, status));
        if let Some(message) = message {
            b = b.on_press(message);
        }
        b
    };
    let footer = row![
        footer_button(
            "ST_STEPS_SAVE_RECIPE",
            (!steps.is_empty()).then_some(Message::SaveRecipe)
        ),
        footer_button("ST_STEPS_LOAD_RECIPE", Some(Message::LoadRecipe)),
    ]
    .spacing(4)
    .padding([4, 8]);
    column![scrollable(content).height(Length::Fill), footer].into()
}

/// One line per edit of an edit-cells step: column, row and new value.
fn edit_lines<'a>(edits: &CellEdits, ui: Ui) -> Element<'a, Message> {
    let t = ui.tokens;
    let line = |label: String| {
        text(label)
            .size(ui.small() - 1.0)
            .color(t.muted_text)
            .wrapping(text::Wrapping::None)
            .into()
    };
    let mut lines: Vec<Element<'a, Message>> = edits
        .iter()
        .take(LISTED_EDITS)
        .map(|(column, row, value)| {
            line(tr_with(
                "ST_STEPS_EDIT_LINE",
                &[
                    ("column", &column),
                    ("n", &(row + 1)),
                    ("value", &abbreviate(&value_text(value), 40)),
                ],
            ))
        })
        .collect();
    if edits.len() > LISTED_EDITS {
        lines.push(line(tr_with(
            "ST_STEPS_MORE_EDITS",
            &[("n", &(edits.len() - LISTED_EDITS))],
        )));
    }
    column(lines).spacing(2).padding([2, 30]).into()
}

/// A one-value array as cell text ("null" for null).
fn value_text(value: &veta_core::arrow::array::ArrayRef) -> String {
    veta_core::arrow::record_batch::RecordBatch::try_from_iter([("value", value.clone())])
        .ok()
        .and_then(|batch| veta_core::display::format_batch(&batch).ok())
        .and_then(|cells| cells.into_iter().next()?.into_iter().next()?)
        .unwrap_or_else(|| "null".into())
}
