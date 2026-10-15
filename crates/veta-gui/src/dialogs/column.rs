//! Add or rename a column, and the column types offered.

use super::*;
use iced::widget::column;

/// Id of the name input, focused when a column dialog opens.
pub const NAME_ID: iced::widget::Id = iced::widget::Id::new("column-name");

/// Types offered when adding a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeChoice {
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
    Timestamp,
}

impl TypeChoice {
    pub const ALL: [TypeChoice; 6] = [
        Self::Text,
        Self::Integer,
        Self::Decimal,
        Self::Boolean,
        Self::Date,
        Self::Timestamp,
    ];

    /// The choice for `data_type`, if it is one offered.
    pub fn from_type(data_type: &DataType) -> Option<Self> {
        Self::ALL.into_iter().find(|c| &c.data_type() == data_type)
    }

    fn data_type(self) -> DataType {
        match self {
            Self::Text => DataType::Utf8,
            Self::Integer => DataType::Int64,
            Self::Decimal => DataType::Float64,
            Self::Boolean => DataType::Boolean,
            Self::Date => DataType::Date32,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
        }
    }
}

impl std::fmt::Display for TypeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text => "Text (string)",
            Self::Integer => "Whole number (int64)",
            Self::Decimal => "Decimal number (float64)",
            Self::Boolean => "True/false (bool)",
            Self::Date => "Date",
            Self::Timestamp => "Date and time (timestamp)",
        })
    }
}

/// Column types offered when adding a column: a plain type, a list, a
/// struct or a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeOption {
    Plain(TypeChoice),
    List,
    Struct,
    Map,
}

impl TypeOption {
    fn all() -> Vec<TypeOption> {
        let mut all: Vec<TypeOption> = TypeChoice::ALL.into_iter().map(Self::Plain).collect();
        all.extend([Self::List, Self::Struct, Self::Map]);
        all
    }
}

impl std::fmt::Display for TypeOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(choice) => choice.fmt(f),
            Self::List => f.write_str("List of values"),
            Self::Struct => f.write_str("Struct (named fields)"),
            Self::Map => f.write_str("Map (keys → values)"),
        }
    }
}

/// Add or rename a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDialog {
    pub mode: ColumnMode,
    name: String,
    choice: TypeOption,
    /// Type of a list's items, or of a map's values.
    item: TypeChoice,
    /// Type of a map's keys.
    key: TypeChoice,
    /// Names and types of a struct's fields.
    fields: Vec<(String, TypeChoice)>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnMode {
    Add {
        at: usize,
    },
    Rename {
        from: String,
    },
    /// Add a field to struct column `column`.
    AddField {
        column: String,
    },
}

#[derive(Debug, Clone)]
pub enum ColumnMessage {
    Name(String),
    Type(TypeOption),
    Item(TypeChoice),
    Key(TypeChoice),
    FieldName(usize, String),
    FieldType(usize, TypeChoice),
    AddField,
    RemoveField(usize),
    Submit,
    Cancel,
}

impl ColumnDialog {
    pub fn add(at: usize) -> Self {
        Self {
            mode: ColumnMode::Add { at },
            name: String::new(),
            choice: TypeOption::Plain(TypeChoice::Text),
            item: TypeChoice::Text,
            key: TypeChoice::Text,
            fields: vec![("field1".into(), TypeChoice::Text)],
            error: None,
        }
    }

    /// Starts with `name` in the name field.
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    /// Starts with the choice matching `data_type`, if it is one offered.
    pub fn with_type(mut self, data_type: &DataType) -> Self {
        if let Some(choice) = TypeChoice::from_type(data_type) {
            self.choice = TypeOption::Plain(choice);
        } else if let DataType::List(item) = data_type
            && let Some(item) = TypeChoice::from_type(item.data_type())
        {
            self.choice = TypeOption::List;
            self.item = item;
        } else if let DataType::Struct(fields) = data_type {
            let fields: Option<Vec<_>> = fields
                .iter()
                .map(|f| TypeChoice::from_type(f.data_type()).map(|t| (f.name().clone(), t)))
                .collect();
            if let Some(fields) = fields.filter(|f| !f.is_empty()) {
                self.choice = TypeOption::Struct;
                self.fields = fields;
            }
        } else if let DataType::Map(entries, _) = data_type
            && let DataType::Struct(kv) = entries.data_type()
            && kv.len() == 2
            && let Some(key) = TypeChoice::from_type(kv[0].data_type())
            && let Some(value) = TypeChoice::from_type(kv[1].data_type())
        {
            self.choice = TypeOption::Map;
            self.key = key;
            self.item = value;
        }
        self
    }

    /// Adds a field to struct column `column`.
    pub fn add_field(column: String) -> Self {
        Self {
            mode: ColumnMode::AddField { column },
            ..Self::add(0)
        }
    }

    pub fn rename(from: String) -> Self {
        Self {
            name: from.clone(),
            mode: ColumnMode::Rename { from },
            ..Self::add(0)
        }
    }

    /// The chosen column type, or why it isn't valid.
    fn data_type(&self) -> Result<DataType, String> {
        Ok(match self.choice {
            TypeOption::Plain(choice) => choice.data_type(),
            TypeOption::List => DataType::new_list(self.item.data_type(), true),
            TypeOption::Map => {
                veta_core::display::map_type(self.key.data_type(), self.item.data_type())
            }
            TypeOption::Struct => {
                let mut fields = Vec::new();
                for (name, choice) in &self.fields {
                    let name = name.trim();
                    if name.is_empty() {
                        return Err("Every field needs a name.".into());
                    }
                    if fields
                        .iter()
                        .any(|f: &veta_core::arrow::datatypes::Field| f.name() == name)
                    {
                        return Err(format!("There are two fields named {name:?}."));
                    }
                    fields.push(veta_core::arrow::datatypes::Field::new(
                        name,
                        choice.data_type(),
                        true,
                    ));
                }
                if fields.is_empty() {
                    return Err("A struct needs at least one field.".into());
                }
                DataType::Struct(fields.into())
            }
        })
    }

    /// Handles a message. Returns the command to run on submit.
    pub fn update(&mut self, message: ColumnMessage) -> Option<Command> {
        self.error = None;
        match message {
            ColumnMessage::Name(name) => self.name = name,
            ColumnMessage::Type(choice) => self.choice = choice,
            ColumnMessage::Item(item) => self.item = item,
            ColumnMessage::Key(key) => self.key = key,
            ColumnMessage::FieldName(i, name) => {
                if let Some(field) = self.fields.get_mut(i) {
                    field.0 = name;
                }
            }
            ColumnMessage::FieldType(i, choice) => {
                if let Some(field) = self.fields.get_mut(i) {
                    field.1 = choice;
                }
            }
            ColumnMessage::AddField => {
                let n = (self.fields.len() + 1..)
                    .map(|n| format!("field{n}"))
                    .find(|name| self.fields.iter().all(|(f, _)| f != name))
                    .unwrap_or_default();
                self.fields.push((n, TypeChoice::Text));
            }
            ColumnMessage::RemoveField(i) => {
                if i < self.fields.len() {
                    self.fields.remove(i);
                }
            }
            ColumnMessage::Cancel => {}
            ColumnMessage::Submit => {
                let name = self.name.trim().to_owned();
                let data_type = match (&self.mode, self.data_type()) {
                    (ColumnMode::Rename { .. }, _) => DataType::Null,
                    (_, Ok(data_type)) => data_type,
                    (_, Err(e)) => {
                        self.error = Some(e);
                        return None;
                    }
                };
                return Some(match &self.mode {
                    ColumnMode::Add { at } => Command::AddColumn {
                        name,
                        data_type,
                        at: *at,
                    },
                    ColumnMode::AddField { column } => Command::AddField {
                        column: column.clone(),
                        name,
                        data_type,
                    },
                    ColumnMode::Rename { from } => Command::RenameColumn {
                        from: from.clone(),
                        to: name,
                    },
                });
            }
        }
        None
    }

    pub fn view(&self, tokens: Tokens) -> Element<'_, ColumnMessage> {
        let title = match &self.mode {
            ColumnMode::Add { .. } => "Add column".to_owned(),
            ColumnMode::Rename { .. } => "Rename column".to_owned(),
            ColumnMode::AddField { column } => format!("Add field to {column}"),
        };
        let action = match &self.mode {
            ColumnMode::Rename { .. } => "Rename",
            _ => "Add",
        };
        let mut body = column![
            text(title).size(20),
            row![
                text("Name").width(70),
                text_input("Name", &self.name)
                    .id(NAME_ID)
                    .on_input(ColumnMessage::Name)
                    .on_submit(ColumnMessage::Submit),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        ]
        .spacing(16);
        if !matches!(self.mode, ColumnMode::Rename { .. }) {
            body = body.push(
                row![
                    text("Type").width(70),
                    pick_list(TypeOption::all(), Some(self.choice), ColumnMessage::Type)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
            match self.choice {
                TypeOption::Plain(_) => {}
                TypeOption::List | TypeOption::Map => {
                    let label = match self.choice {
                        TypeOption::List => "Items",
                        _ => "Values",
                    };
                    if self.choice == TypeOption::Map {
                        body = body.push(
                            row![
                                text("Keys").width(70),
                                pick_list(TypeChoice::ALL, Some(self.key), ColumnMessage::Key)
                                    .width(Length::Fill),
                            ]
                            .spacing(12)
                            .align_y(Alignment::Center),
                        );
                    }
                    body = body.push(
                        row![
                            text(label).width(70),
                            pick_list(TypeChoice::ALL, Some(self.item), ColumnMessage::Item)
                                .width(Length::Fill),
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center),
                    );
                }
                TypeOption::Struct => {
                    let fields = column(self.fields.iter().enumerate().map(|(i, (name, t))| {
                        row![
                            text_input("Field name", name)
                                .on_input(move |n| ColumnMessage::FieldName(i, n))
                                .on_submit(ColumnMessage::Submit)
                                .width(Length::FillPortion(2)),
                            pick_list(TypeChoice::ALL, Some(*t), move |t| {
                                ColumnMessage::FieldType(i, t)
                            })
                            .width(Length::FillPortion(3)),
                            button(text("✕"))
                                .style(button::text)
                                .on_press(ColumnMessage::RemoveField(i)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .into()
                    }))
                    .spacing(6);
                    body = body.push(text("Fields"));
                    body = body.push(iced::widget::container(scrollable(fields)).max_height(220.0));
                    body = body.push(
                        button(text("Add field"))
                            .style(button::secondary)
                            .on_press(ColumnMessage::AddField),
                    );
                }
            }
            let note = match self.mode {
                ColumnMode::AddField { .. } => "The new field starts empty (null) in every row.",
                _ if self.choice == TypeOption::Map => {
                    "The new column starts empty (null). Add entries to each row in the grid."
                }
                _ => "The new column starts empty (null).",
            };
            body = body.push(text(note).color(tokens.muted_text));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(iced::Color::from_rgb8(0xd1, 0x43, 0x43)));
        }
        body = body.push(buttons(
            action,
            ColumnMessage::Cancel,
            ColumnMessage::Submit,
        ));
        card(body.into(), tokens, 420.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_rename_build_commands() {
        let mut add = ColumnDialog::add(2);
        add.update(ColumnMessage::Name(" total ".into()));
        add.update(ColumnMessage::Type(TypeOption::Plain(TypeChoice::Decimal)));
        assert_eq!(
            add.update(ColumnMessage::Submit),
            Some(Command::AddColumn {
                name: "total".into(),
                data_type: DataType::Float64,
                at: 2
            })
        );
        let mut map = ColumnDialog::add(0);
        map.update(ColumnMessage::Name("scores".into()));
        map.update(ColumnMessage::Type(TypeOption::Map));
        map.update(ColumnMessage::Key(TypeChoice::Date));
        map.update(ColumnMessage::Item(TypeChoice::Integer));
        let scores = veta_core::display::map_type(DataType::Date32, DataType::Int64);
        assert_eq!(
            map.update(ColumnMessage::Submit),
            Some(Command::AddColumn {
                name: "scores".into(),
                data_type: scores.clone(),
                at: 0
            })
        );
        let again = ColumnDialog::add(0).with_type(&scores);
        assert_eq!(
            (again.choice, again.key, again.item),
            (TypeOption::Map, TypeChoice::Date, TypeChoice::Integer)
        );
        let mut rename = ColumnDialog::rename("a".into());
        rename.update(ColumnMessage::Name("b".into()));
        assert_eq!(
            rename.update(ColumnMessage::Submit),
            Some(Command::RenameColumn {
                from: "a".into(),
                to: "b".into()
            })
        );
    }
}
