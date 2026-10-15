//! Yes/no questions: change a step, load a recipe, save before closing.

use super::*;
use iced::widget::column;

/// Asked before changing a step when later steps refer to rows by position.
pub fn confirm_step<'a>(tokens: Tokens) -> Element<'a, crate::Message> {
    use crate::Message;
    let body = column![
        text("Change this step?").size(20),
        text(
            "Later steps edit, insert or delete rows by their position. After this \
             change they may apply to different rows."
        )
        .color(tokens.muted_text),
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text("Cancel"))
                .style(button::secondary)
                .on_press(Message::ConfirmStep(false)),
            button(text("Change it")).on_press(Message::ConfirmStep(true)),
        ]
        .spacing(8),
    ]
    .spacing(14);
    card(body.into(), tokens, 460.0)
}

/// Asked when loading a recipe into a document that has `steps` steps.
pub fn confirm_recipe<'a>(steps: usize, tokens: Tokens) -> Element<'a, crate::Message> {
    use crate::Message;
    let have = if steps == 1 {
        "This file already has 1 applied step.".to_owned()
    } else {
        format!("This file already has {steps} applied steps.")
    };
    let body = column![
        text("Load recipe").size(20),
        text(format!(
            "{have} Replace them with the recipe's, which then applies to the original \
             data, or add the recipe's steps after them?"
        ))
        .color(tokens.muted_text),
        row![
            iced::widget::Space::new().width(Length::Fill),
            button(text("Cancel"))
                .style(button::secondary)
                .on_press(Message::RecipeChoice(None)),
            button(text("Add after"))
                .style(button::secondary)
                .on_press(Message::RecipeChoice(Some(false))),
            button(text("Replace")).on_press(Message::RecipeChoice(Some(true))),
        ]
        .spacing(8),
    ]
    .spacing(14);
    card(body.into(), tokens, 480.0)
}

/// "Save changes to X?" when closing a modified document.
pub fn confirm_close<'a>(name: String, tokens: Tokens) -> Element<'a, crate::Message> {
    use crate::{CloseAnswer, Message};
    let body = column![
        text(format!("Save changes to {name}?")).size(20),
        text("Your changes will be lost if you don't save them.").color(tokens.muted_text),
        row![
            button(text("Don't save"))
                .style(button::danger)
                .on_press(Message::CloseAnswer(CloseAnswer::Discard)),
            iced::widget::Space::new().width(Length::Fill),
            button(text("Cancel"))
                .style(button::secondary)
                .on_press(Message::CloseAnswer(CloseAnswer::Cancel)),
            button(text("Save")).on_press(Message::CloseAnswer(CloseAnswer::Save)),
        ]
        .spacing(8),
    ]
    .spacing(14);
    card(body.into(), tokens, 440.0)
}
