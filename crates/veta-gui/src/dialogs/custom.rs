//! Add a column computed from an expression.

use super::*;
use iced::widget::column;

/// Add a column computed from an expression.
#[derive(Debug, Clone)]
pub struct CustomDialog {
    /// The input of the new column's step.
    schema: SchemaRef,
    name: String,
    expression: String,
    at: usize,
    /// Type of the expression, or why it is invalid.
    check: Result<String, String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CustomMessage {
    Name(String),
    Expression(String),
    Apply,
    Cancel,
}

/// Id of the expression input.
pub const EXPRESSION_ID: iced::widget::Id = iced::widget::Id::new("expression");

impl CustomDialog {
    pub fn new(schema: SchemaRef, custom: CustomColumn) -> Self {
        let mut dialog = Self {
            schema,
            name: custom.name,
            expression: custom.expression,
            at: custom.at,
            check: Ok(String::new()),
            error: None,
        };
        dialog.recheck();
        dialog
    }

    fn custom(&self) -> CustomColumn {
        CustomColumn {
            name: self.name.trim().to_owned(),
            expression: self.expression.clone(),
            at: self.at,
        }
    }

    fn recheck(&mut self) {
        self.check = if self.expression.trim().is_empty() {
            Err(String::new())
        } else {
            self.custom()
                .data_type(&self.schema)
                .map(|t| type_name(&t))
                .map_err(|e| e.to_string())
        };
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, message: CustomMessage) -> Option<CustomColumn> {
        self.error = None;
        match message {
            CustomMessage::Name(n) => self.name = n,
            CustomMessage::Expression(e) => {
                self.expression = e;
                self.recheck();
            }
            CustomMessage::Cancel => {}
            CustomMessage::Apply => return Some(self.custom()),
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, CustomMessage> {
        let danger = iced::Color::from_rgb8(0xd1, 0x43, 0x43);
        let status: Element<'_, CustomMessage> = match &self.check {
            Ok(t) => text(format!("Result: {t}")).color(tokens.muted_text).into(),
            Err(e) if e.is_empty() => text("").into(),
            Err(e) => text(e).color(danger).into(),
        };
        let columns: Vec<String> = self
            .schema
            .fields()
            .iter()
            .map(|f| {
                if f.name().chars().all(|c| c.is_alphanumeric() || c == '_') {
                    f.name().clone()
                } else {
                    format!("[{}]", f.name())
                }
            })
            .collect();
        let mut body = column![
            text("Custom column").size(20),
            row![
                text("Name").width(90),
                text_input("Column name", &self.name)
                    .id(NAME_ID)
                    .on_input(CustomMessage::Name)
                    .on_submit(CustomMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                text("Formula").width(90),
                text_input("e.g. [Unit price] * qty", &self.expression)
                    .id(EXPRESSION_ID)
                    .on_input(CustomMessage::Expression)
                    .on_submit(CustomMessage::Apply),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            status,
            text(format!("Columns: {}", columns.join(", ")))
                .size(12)
                .color(tokens.muted_text),
            text(expression_help()).size(12).color(tokens.muted_text),
        ]
        .spacing(12);
        if let Some(error) = &self.error {
            body = body.push(text(error).color(danger));
        }
        body = body.push(buttons("OK", CustomMessage::Cancel, CustomMessage::Apply));
        card(body.into(), tokens, 620.0)
    }
}

impl StepDialog for CustomDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Custom(message) = message else {
            return None;
        };
        CustomDialog::update(self, message).map(Step::CustomColumn)
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Custom(CustomMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        CustomDialog::view(self, tokens).map(StepMessage::Custom)
    }

    fn set_error(&mut self, error: String) -> Option<String> {
        CustomDialog::set_error(self, error);
        None
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
