//! Change a column's type.

use super::*;
use iced::widget::column;

/// A target type offered when changing a column's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastTarget {
    Text,
    Int64,
    Int32,
    Float64,
    Float32,
    Boolean,
    Date,
    Timestamp,
    Time,
    /// Any type, typed by name.
    Other,
}

impl CastTarget {
    pub const ALL: [CastTarget; 10] = [
        Self::Text,
        Self::Int64,
        Self::Int32,
        Self::Float64,
        Self::Float32,
        Self::Boolean,
        Self::Date,
        Self::Timestamp,
        Self::Time,
        Self::Other,
    ];

    pub(super) fn data_type(self) -> Option<DataType> {
        Some(match self {
            Self::Text => DataType::Utf8,
            Self::Int64 => DataType::Int64,
            Self::Int32 => DataType::Int32,
            Self::Float64 => DataType::Float64,
            Self::Float32 => DataType::Float32,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
            Self::Time => DataType::Time64(TimeUnit::Microsecond),
            Self::Other => return None,
        })
    }
}

impl std::fmt::Display for CastTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&tr(match self {
            Self::Text => "ST_TYPE_TEXT",
            Self::Int64 => "ST_TYPE_INTEGER",
            Self::Int32 => "ST_TYPE_INT32",
            Self::Float64 => "ST_TYPE_DECIMAL",
            Self::Float32 => "ST_TYPE_FLOAT32",
            Self::Boolean => "ST_TYPE_BOOLEAN",
            Self::Date => "ST_TYPE_DATE",
            Self::Timestamp => "ST_TYPE_TIMESTAMP",
            Self::Time => "ST_TYPE_TIME",
            Self::Other => "ST_TYPE_OTHER",
        }))
    }
}

/// A separator choice for numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    None,
    Dot,
    Comma,
    Space,
    Apostrophe,
}

impl Separator {
    fn char(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::Dot => Some('.'),
            Self::Comma => Some(','),
            Self::Space => Some(' '),
            Self::Apostrophe => Some('\''),
        }
    }

    fn from_char(c: Option<char>) -> Self {
        match c {
            Some('.') => Self::Dot,
            Some(',') => Self::Comma,
            Some(' ') => Self::Space,
            Some('\'') => Self::Apostrophe,
            _ => Self::None,
        }
    }
}

impl std::fmt::Display for Separator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&tr(match self {
            Self::None => "ST_DLG_CAST_SEPARATOR_NONE",
            Self::Dot => "ST_DLG_CAST_SEPARATOR_DOT",
            Self::Comma => "ST_DLG_CAST_SEPARATOR_COMMA",
            Self::Space => "ST_DLG_CAST_SEPARATOR_SPACE",
            Self::Apostrophe => "ST_DLG_CAST_SEPARATOR_APOSTROPHE",
        }))
    }
}

/// Change a column's type (Power Query's "Change Type … Using Locale").
#[derive(Debug, Clone)]
pub struct CastDialog {
    column: String,
    from_text: bool,
    /// The column's values are numbers (text output can use separators).
    from_number: bool,
    target: CastTarget,
    /// Type name typed for "Other type…", e.g. `decimal(10, 2)`.
    other: String,
    decimal: Separator,
    thousands: Separator,
    format: String,
    on_error: OnError,
    /// The first rows of the column, to preview failures on.
    sample: ArrayRef,
    preview: CastPreview,
}

#[derive(Debug, Clone)]
pub enum CastMessage {
    Type(CastTarget),
    Other(String),
    Decimal(Separator),
    Thousands(Separator),
    Format(String),
    OnError(OnError),
    Apply,
    Cancel,
}

impl CastDialog {
    /// A dialog for `column`, whose first values are `sample`. `cast` fills
    /// it in when editing a step.
    pub fn new(column: String, sample: ArrayRef, cast: Option<&Cast>) -> Self {
        let from_text = is_text(sample.data_type());
        let from_number = sample.data_type().is_numeric();
        let options = cast.map(|c| c.options.clone()).unwrap_or_default();
        let (target, other) = match cast {
            Some(c) => match CastTarget::ALL
                .iter()
                .find(|t| t.data_type().as_ref() == Some(&c.to))
            {
                Some(t) => (*t, String::new()),
                None => (CastTarget::Other, veta_core::display::type_name(&c.to)),
            },
            None if from_text => (CastTarget::Float64, String::new()),
            None => (CastTarget::Text, String::new()),
        };
        let mut dialog = Self {
            column,
            from_text,
            from_number,
            target,
            other,
            decimal: Separator::from_char(Some(options.decimal)),
            thousands: Separator::from_char(options.thousands),
            format: options.format.unwrap_or_default(),
            on_error: options.on_error,
            sample,
            preview: CastPreview::default(),
        };
        dialog.refresh();
        dialog
    }

    /// The chosen type; `None` while "Other type…" holds no valid name.
    fn data_type(&self) -> Option<DataType> {
        match self.target {
            CastTarget::Other => veta_core::display::parse_type_name(&self.other),
            target => target.data_type(),
        }
    }

    fn uses_format(&self) -> bool {
        self.from_text
            && matches!(
                self.data_type(),
                Some(DataType::Date32 | DataType::Timestamp(..))
            )
    }

    pub(super) fn uses_separators(&self) -> bool {
        let to = self.data_type();
        let to_number = to.as_ref().is_some_and(DataType::is_numeric);
        let to_text = to.as_ref().is_some_and(is_text);
        (self.from_text && to_number) || (self.from_number && to_text)
    }

    fn cast(&self) -> Option<Cast> {
        let format = self.format.trim();
        Some(Cast {
            column: self.column.clone(),
            to: self.data_type()?,
            options: CastOptions {
                decimal: self.decimal.char().unwrap_or('.'),
                thousands: self.thousands.char(),
                format: (self.uses_format() && !format.is_empty()).then(|| format.to_owned()),
                on_error: self.on_error,
            },
        })
    }

    fn refresh(&mut self) {
        self.preview = match self.cast() {
            Some(cast) => cast.preview(&self.sample),
            None => CastPreview::default(),
        };
    }

    /// Handles a message. Returns the cast to apply on submit.
    pub fn update(&mut self, message: CastMessage) -> Option<Cast> {
        match message {
            CastMessage::Type(target) => {
                if target == CastTarget::Other && self.other.is_empty() {
                    self.other = self
                        .data_type()
                        .map(|t| veta_core::display::type_name(&t))
                        .unwrap_or_default();
                }
                self.target = target;
            }
            CastMessage::Other(name) => self.other = name,
            CastMessage::Decimal(s) => self.decimal = s,
            CastMessage::Thousands(s) => self.thousands = s,
            CastMessage::Format(f) => self.format = f,
            CastMessage::OnError(e) => self.on_error = e,
            CastMessage::Cancel => return None,
            CastMessage::Apply => return self.cast(),
        }
        self.refresh();
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, CastMessage> {
        const LABEL: f32 = 130.0;
        let labelled = |label: &str, control: Element<'static, CastMessage>| {
            row![text(tr(label)).width(LABEL), control]
                .spacing(12)
                .align_y(Alignment::Center)
        };
        let mut body = column![
            text(tr_with("ST_DLG_CAST_TITLE", &[("column", &self.column)])).size(20),
            labelled(
                "ST_DLG_CAST_TYPE",
                pick_list(CastTarget::ALL, Some(self.target), CastMessage::Type)
                    .width(Length::Fill)
                    .into()
            ),
        ]
        .spacing(14);
        if self.target == CastTarget::Other {
            body = body.push(labelled(
                "ST_DLG_CAST_TYPE_NAME",
                text_input(&tr("ST_DLG_CAST_TYPE_NAME_EXAMPLE"), &self.other)
                    .on_input(CastMessage::Other)
                    .on_submit(CastMessage::Apply)
                    .into(),
            ));
            if self.data_type().is_none() {
                body = body.push(
                    text(tr("ST_DLG_CAST_TYPE_NAMES"))
                        .size(12)
                        .color(tokens.muted_text),
                );
            }
        }
        if self.uses_separators() {
            body = body.push(labelled(
                "ST_DLG_CAST_DECIMAL",
                pick_list(
                    [Separator::Dot, Separator::Comma],
                    Some(self.decimal),
                    CastMessage::Decimal,
                )
                .width(Length::Fill)
                .into(),
            ));
            body = body.push(labelled(
                "ST_DLG_CAST_THOUSANDS",
                pick_list(
                    [
                        Separator::None,
                        Separator::Comma,
                        Separator::Dot,
                        Separator::Space,
                        Separator::Apostrophe,
                    ],
                    Some(self.thousands),
                    CastMessage::Thousands,
                )
                .width(Length::Fill)
                .into(),
            ));
        }
        if self.uses_format() {
            body = body.push(labelled(
                "ST_DLG_CAST_FORMAT",
                text_input(&tr("ST_DLG_CAST_FORMAT_AUTOMATIC"), &self.format)
                    .on_input(CastMessage::Format)
                    .on_submit(CastMessage::Apply)
                    .into(),
            ));
            body = body.push(
                text(tr("ST_HELP_FORMAT_CODES"))
                    .size(12)
                    .color(tokens.muted_text),
            );
        }
        body = body.push(labelled(
            "ST_DLG_CAST_INVALID",
            pick_list(OnError::ALL, Some(self.on_error), CastMessage::OnError)
                .width(Length::Fill)
                .into(),
        ));
        let p = &self.preview;
        let summary = if self.data_type().is_none() {
            tr("ST_DLG_CAST_ENTER_TYPE")
        } else if p.checked == 0 {
            tr("ST_DLG_CAST_NO_ROWS")
        } else if p.failed == 0 {
            tr_with("ST_DLG_CAST_ALL_CONVERT", &[("n", &p.checked)])
        } else {
            let examples: Vec<String> = p.examples.iter().map(|e| format!("{e:?}")).collect();
            tr_with(
                "ST_DLG_CAST_SOME_FAIL",
                &[
                    ("failed", &p.failed),
                    ("n", &p.checked),
                    ("examples", &examples.join(", ")),
                ],
            )
        };
        let color = if p.failed == 0 {
            tokens.muted_text
        } else {
            iced::Color::from_rgb8(0xd1, 0x43, 0x43)
        };
        body = body.push(text(summary).color(color));
        body = body.push(buttons(
            "ST_COMMON_OK",
            CastMessage::Cancel,
            CastMessage::Apply,
        ));
        card(body.into(), tokens, 520.0)
    }
}

impl StepDialog for CastDialog {
    fn update(&mut self, message: StepMessage) -> Option<Step> {
        let StepMessage::Cast(message) = message else {
            return None;
        };
        CastDialog::update(self, message).map(Step::Cast)
    }

    fn cancels(&self, message: &StepMessage) -> bool {
        matches!(message, StepMessage::Cast(CastMessage::Cancel))
    }

    fn view(&self, tokens: Tokens) -> Element<'_, StepMessage> {
        CastDialog::view(self, tokens).map(StepMessage::Cast)
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_dialog_other_types_and_separators() {
        let sample: ArrayRef =
            std::sync::Arc::new(veta_core::arrow::array::Float64Array::from(vec![1234.5]));
        let mut dialog = CastDialog::new("n".into(), sample, None);
        assert_eq!(dialog.target, CastTarget::Text);
        assert!(dialog.uses_separators(), "numbers to text use separators");
        dialog.update(CastMessage::Decimal(Separator::Comma));
        dialog.update(CastMessage::Type(CastTarget::Other));
        assert_eq!(dialog.other, "string");
        dialog.update(CastMessage::Other("decimal(10".into()));
        assert_eq!(dialog.update(CastMessage::Apply), None, "not a type yet");
        dialog.update(CastMessage::Other("decimal(10, 2)".into()));
        let cast = dialog.update(CastMessage::Apply).unwrap();
        assert_eq!(cast.to, DataType::Decimal128(10, 2));
        // Editing a step with a type not in the list shows its name.
        let edit = CastDialog::new("n".into(), dialog.sample.clone(), Some(&cast));
        assert_eq!(
            (edit.target, edit.other.as_str()),
            (CastTarget::Other, "decimal(10, 2)")
        );
    }
}
