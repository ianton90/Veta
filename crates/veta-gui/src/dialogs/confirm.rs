//! Yes/no questions: change a step, load a recipe, save before closing.

use super::*;
use iced::widget::column;

/// Asked before changing a step when later steps refer to rows by position.
pub fn confirm_step<'a>(tokens: Tokens) -> Element<'a, crate::app::Message> {
    use crate::app::Message;
    let body = column![
        text(tr("ST_DLG_CONFIRM_STEP_TITLE")).size(20),
        text(tr("ST_DLG_CONFIRM_STEP_TEXT")).color(tokens.muted_text),
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text(tr("ST_COMMON_CANCEL")))
                .style(button::secondary)
                .on_press(Message::ConfirmStep(false)),
            button(text(tr("ST_DLG_CONFIRM_STEP_CHANGE"))).on_press(Message::ConfirmStep(true)),
        ]
        .spacing(8),
    ]
    .spacing(14);
    card(body.into(), tokens, 460.0)
}

/// Asked when loading a recipe into a document that has `steps` steps.
pub fn confirm_recipe<'a>(steps: usize, tokens: Tokens) -> Element<'a, crate::app::Message> {
    use crate::app::Message;
    let question = if steps == 1 {
        tr("ST_DLG_CONFIRM_RECIPE_ONE")
    } else {
        tr_with("ST_DLG_CONFIRM_RECIPE_MANY", &[("n", &steps)])
    };
    let body = column![
        text(tr("ST_DLG_CONFIRM_RECIPE_TITLE")).size(20),
        text(question).color(tokens.muted_text),
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text(tr("ST_COMMON_CANCEL")))
                .style(button::secondary)
                .on_press(Message::RecipeChoice(None)),
            button(text(tr("ST_DLG_CONFIRM_RECIPE_ADD")))
                .style(button::secondary)
                .on_press(Message::RecipeChoice(Some(false))),
            button(text(tr("ST_DLG_CONFIRM_RECIPE_REPLACE")))
                .on_press(Message::RecipeChoice(Some(true))),
        ]
        .spacing(8),
    ]
    .spacing(14);
    card(body.into(), tokens, 480.0)
}

/// "Save changes to X?" when closing a modified document.
pub fn confirm_close<'a>(name: String, tokens: Tokens) -> Element<'a, crate::app::Message> {
    use crate::app::{CloseAnswer, Message};
    let body = column![
        text(tr_with("ST_DLG_CONFIRM_CLOSE_TITLE", &[("name", &name)])).size(20),
        text(tr("ST_DLG_CONFIRM_CLOSE_TEXT")).color(tokens.muted_text),
        row![
            button(text(tr("ST_DLG_CONFIRM_CLOSE_DISCARD")))
                .style(button::danger)
                .on_press(Message::CloseAnswer(CloseAnswer::Discard)),
            iced::widget::Space::new().width(Length::Fill),
            button(text(tr("ST_COMMON_CANCEL")))
                .style(button::secondary)
                .on_press(Message::CloseAnswer(CloseAnswer::Cancel)),
            button(text(tr("ST_COMMON_SAVE"))).on_press(Message::CloseAnswer(CloseAnswer::Save)),
        ]
        .spacing(8),
    ]
    .spacing(14);
    card(body.into(), tokens, 440.0)
}
