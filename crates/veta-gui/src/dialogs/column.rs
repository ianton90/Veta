//! Add or rename a column, and the column types offered: plain types, and
//! lists, structs and maps of any of them, nested to any depth.

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
        f.write_str(&tr(match self {
            Self::Text => "ST_TYPE_TEXT",
            Self::Integer => "ST_TYPE_INTEGER",
            Self::Decimal => "ST_TYPE_DECIMAL",
            Self::Boolean => "ST_TYPE_BOOLEAN",
            Self::Date => "ST_TYPE_DATE",
            Self::Timestamp => "ST_TYPE_TIMESTAMP",
        }))
    }
}

/// The kinds of type offered at each level: a plain type, a list, a struct
/// or a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeOption {
    Plain(TypeChoice),
    List,
    FixedList,
    Struct,
    Map,
}

impl TypeOption {
    fn all() -> Vec<TypeOption> {
        let mut all: Vec<TypeOption> = TypeChoice::ALL.into_iter().map(Self::Plain).collect();
        all.extend([Self::List, Self::FixedList, Self::Struct, Self::Map]);
        all
    }
}

impl std::fmt::Display for TypeOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(choice) => choice.fmt(f),
            Self::List => f.write_str(&tr("ST_TYPE_LIST")),
            Self::FixedList => f.write_str(&tr("ST_TYPE_FIXED_LIST")),
            Self::Struct => f.write_str(&tr("ST_TYPE_STRUCT")),
            Self::Map => f.write_str(&tr("ST_TYPE_MAP")),
        }
    }
}

/// A column type being chosen: a plain type, or a list, struct or map of
/// other such types.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeSpec {
    Plain(TypeChoice),
    List(Box<TypeSpec>),
    /// Item type and number of items (as typed).
    FixedList(Box<TypeSpec>, String),
    Struct(Vec<(String, TypeSpec)>),
    /// Key type and value type.
    Map(TypeChoice, Box<TypeSpec>),
}

/// A step from a type to one inside it, in the type pickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeStep {
    /// A list's items, or a map's values.
    Item,
    /// Field `n` of a struct.
    Field(usize),
}

/// Steps from the column's type down to a type inside it.
pub type TypePath = Vec<TypeStep>;

impl TypeSpec {
    /// A new type of kind `option`. Lists and maps keep what they hold when
    /// switched to each other (a new fixed-size list has 3 items).
    fn new(option: TypeOption, old: &TypeSpec) -> Self {
        let inner = match old {
            Self::List(item) | Self::FixedList(item, _) | Self::Map(_, item) => (**item).clone(),
            _ => Self::Plain(TypeChoice::Text),
        };
        match option {
            TypeOption::Plain(choice) => Self::Plain(choice),
            TypeOption::List => Self::List(Box::new(inner)),
            TypeOption::FixedList => Self::FixedList(Box::new(inner), "3".into()),
            TypeOption::Map => Self::Map(TypeChoice::Text, Box::new(inner)),
            TypeOption::Struct => {
                Self::Struct(vec![("field1".into(), Self::Plain(TypeChoice::Text))])
            }
        }
    }

    fn option(&self) -> TypeOption {
        match self {
            Self::Plain(choice) => TypeOption::Plain(*choice),
            Self::List(_) => TypeOption::List,
            Self::FixedList(..) => TypeOption::FixedList,
            Self::Struct(_) => TypeOption::Struct,
            Self::Map(..) => TypeOption::Map,
        }
    }

    /// The type at `path` inside this one.
    fn get_mut(&mut self, path: &[TypeStep]) -> Option<&mut TypeSpec> {
        let Some((step, rest)) = path.split_first() else {
            return Some(self);
        };
        match (self, step) {
            (Self::List(item) | Self::FixedList(item, _) | Self::Map(_, item), TypeStep::Item) => {
                item.get_mut(rest)
            }
            (Self::Struct(fields), TypeStep::Field(i)) => fields.get_mut(*i)?.1.get_mut(rest),
            _ => None,
        }
    }

    /// The type chosen, or why it isn't valid.
    fn data_type(&self) -> Result<DataType, String> {
        Ok(match self {
            Self::Plain(choice) => choice.data_type(),
            Self::List(item) => DataType::new_list(item.data_type()?, true),
            Self::FixedList(item, len) => {
                let len: i32 = len
                    .trim()
                    .parse()
                    .ok()
                    .filter(|n| (1..=1_000_000).contains(n))
                    .ok_or_else(|| tr("ST_DLG_COLUMN_FIXED_LIST_LENGTH"))?;
                DataType::FixedSizeList(
                    std::sync::Arc::new(veta_core::arrow::datatypes::Field::new(
                        "item",
                        item.data_type()?,
                        true,
                    )),
                    len,
                )
            }
            Self::Map(key, value) => {
                veta_core::display::map_type(key.data_type(), value.data_type()?)
            }
            Self::Struct(fields) => {
                let mut out: Vec<veta_core::arrow::datatypes::Field> = Vec::new();
                for (name, spec) in fields {
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(tr("ST_DLG_COLUMN_FIELD_NEEDS_NAME"));
                    }
                    if out.iter().any(|f| f.name() == name) {
                        return Err(tr_with("ST_DLG_COLUMN_FIELD_TWICE", &[("name", &name)]));
                    }
                    out.push(veta_core::arrow::datatypes::Field::new(
                        name,
                        spec.data_type()?,
                        true,
                    ));
                }
                if out.is_empty() {
                    return Err(tr("ST_DLG_COLUMN_STRUCT_NO_FIELDS"));
                }
                DataType::Struct(out.into())
            }
        })
    }

    /// The choice for `data_type`, if it is made of types offered.
    fn from_type(data_type: &DataType) -> Option<Self> {
        if let Some(choice) = TypeChoice::from_type(data_type) {
            return Some(Self::Plain(choice));
        }
        match data_type {
            DataType::List(item) => Some(Self::List(Box::new(Self::from_type(item.data_type())?))),
            DataType::FixedSizeList(item, n) => Some(Self::FixedList(
                Box::new(Self::from_type(item.data_type())?),
                n.to_string(),
            )),
            DataType::Struct(fields) if !fields.is_empty() => Some(Self::Struct(
                fields
                    .iter()
                    .map(|f| Some((f.name().clone(), Self::from_type(f.data_type())?)))
                    .collect::<Option<_>>()?,
            )),
            DataType::Map(entries, _) => match entries.data_type() {
                DataType::Struct(kv) if kv.len() == 2 => Some(Self::Map(
                    TypeChoice::from_type(kv[0].data_type())?,
                    Box::new(Self::from_type(kv[1].data_type())?),
                )),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Add or rename a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDialog {
    pub mode: ColumnMode,
    name: String,
    spec: TypeSpec,
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
    /// Add a field to the struct at `path` in column `column` (empty: the
    /// column).
    AddField {
        column: String,
        path: Vec<veta_core::display::PathPart>,
    },
}

#[derive(Debug, Clone)]
pub enum ColumnMessage {
    Name(String),
    /// The kind of the type at a path (empty: the column's type).
    Type(TypePath, TypeOption),
    /// The key type of the map at a path.
    Key(TypePath, TypeChoice),
    /// The length of the fixed-size list at a path, as typed.
    Length(TypePath, String),
    /// The name of field `n` of the struct at a path.
    FieldName(TypePath, usize, String),
    AddField(TypePath),
    RemoveField(TypePath, usize),
    Submit,
    Cancel,
}

impl ColumnDialog {
    pub fn add(at: usize) -> Self {
        Self {
            mode: ColumnMode::Add { at },
            name: String::new(),
            spec: TypeSpec::Plain(TypeChoice::Text),
            error: None,
        }
    }

    /// Starts with `name` in the name field.
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    /// Starts with the choice matching `data_type`, if it is made of types
    /// offered.
    pub fn with_type(mut self, data_type: &DataType) -> Self {
        if let Some(spec) = TypeSpec::from_type(data_type) {
            self.spec = spec;
        }
        self
    }

    /// Adds a field to the struct at `path` in column `column`.
    pub fn add_field(column: String, path: Vec<veta_core::display::PathPart>) -> Self {
        Self {
            mode: ColumnMode::AddField { column, path },
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

    /// Handles a message. Returns the command to run on submit.
    pub fn update(&mut self, message: ColumnMessage) -> Option<Command> {
        self.error = None;
        match message {
            ColumnMessage::Name(name) => self.name = name,
            ColumnMessage::Type(path, option) => {
                if let Some(spec) = self.spec.get_mut(&path)
                    && spec.option() != option
                {
                    *spec = TypeSpec::new(option, spec);
                }
            }
            ColumnMessage::Key(path, key) => {
                if let Some(TypeSpec::Map(k, _)) = self.spec.get_mut(&path) {
                    *k = key;
                }
            }
            ColumnMessage::Length(path, length) => {
                if let Some(TypeSpec::FixedList(_, len)) = self.spec.get_mut(&path) {
                    *len = length;
                }
            }
            ColumnMessage::FieldName(path, i, name) => {
                if let Some(TypeSpec::Struct(fields)) = self.spec.get_mut(&path)
                    && let Some(field) = fields.get_mut(i)
                {
                    field.0 = name;
                }
            }
            ColumnMessage::AddField(path) => {
                if let Some(TypeSpec::Struct(fields)) = self.spec.get_mut(&path) {
                    let n = (fields.len() + 1..)
                        .map(|n| format!("field{n}"))
                        .find(|name| fields.iter().all(|(f, _)| f != name))
                        .unwrap_or_default();
                    fields.push((n, TypeSpec::Plain(TypeChoice::Text)));
                }
            }
            ColumnMessage::RemoveField(path, i) => {
                if let Some(TypeSpec::Struct(fields)) = self.spec.get_mut(&path)
                    && i < fields.len()
                    && fields.len() > 1
                {
                    fields.remove(i);
                }
            }
            ColumnMessage::Cancel => {}
            ColumnMessage::Submit => {
                let name = self.name.trim().to_owned();
                let data_type = match (&self.mode, self.spec.data_type()) {
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
                    ColumnMode::AddField { column, path } => Command::AddField {
                        column: column.clone(),
                        path: path.clone(),
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
            ColumnMode::Add { .. } => tr("ST_DLG_COLUMN_ADD_TITLE"),
            ColumnMode::Rename { .. } => tr("ST_DLG_COLUMN_RENAME_TITLE"),
            ColumnMode::AddField { column, path } => {
                let path = format!("{column}{}", veta_core::steps::type_path(path));
                tr_with("ST_DLG_COLUMN_ADD_FIELD_TITLE", &[("path", &path)])
            }
        };
        let action = match &self.mode {
            ColumnMode::Rename { .. } => "ST_DLG_COLUMN_RENAME",
            _ => "ST_DLG_COLUMN_ADD",
        };
        let mut body = column![
            text(title).size(20),
            row![
                text(tr("ST_DLG_COLUMN_NAME")).width(70),
                text_input(&tr("ST_DLG_COLUMN_NAME"), &self.name)
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
                    text(tr("ST_DLG_COLUMN_TYPE")).width(70),
                    picker(&self.spec, Vec::new()),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
            if let Some(inner) = inner_pickers(&self.spec, &[]) {
                body = body
                    .push(iced::widget::container(scrollable(inner).spacing(4)).max_height(320.0));
            }
            let note = match self.mode {
                ColumnMode::AddField { .. } => "ST_DLG_COLUMN_FIELD_EMPTY",
                _ if matches!(self.spec, TypeSpec::Map(..)) => "ST_DLG_COLUMN_MAP_EMPTY",
                _ => "ST_DLG_COLUMN_EMPTY",
            };
            body = body.push(text(tr(note)).color(tokens.muted_text));
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).style(iced::widget::text::danger));
        }
        body = body.push(buttons(
            action,
            ColumnMessage::Cancel,
            ColumnMessage::Submit,
        ));
        card(body.into(), tokens, 480.0)
    }
}

/// The kind picker for the type at `path`.
fn picker(spec: &TypeSpec, path: TypePath) -> Element<'_, ColumnMessage> {
    pick_list(TypeOption::all(), Some(spec.option()), move |option| {
        ColumnMessage::Type(path.clone(), option)
    })
    .width(Length::Fill)
    .into()
}

/// The pickers for what the type at `path` holds (a list's items, a map's
/// keys and values, a struct's fields), indented under it; `None` for a
/// plain type.
fn inner_pickers<'a>(spec: &'a TypeSpec, path: &[TypeStep]) -> Option<Element<'a, ColumnMessage>> {
    let child = |step| [path, &[step]].concat();
    let labelled =
        |label: &str, picker: Element<'a, ColumnMessage>| -> Element<'a, ColumnMessage> {
            row![text(tr(label)).width(58), picker]
                .spacing(12)
                .align_y(Alignment::Center)
                .into()
        };
    let mut rows = column![].spacing(6);
    match spec {
        TypeSpec::Plain(_) => return None,
        TypeSpec::FixedList(item, len) => {
            let at = path.to_vec();
            let length = text_input("3", len)
                .on_input(move |l| ColumnMessage::Length(at.clone(), l))
                .on_submit(ColumnMessage::Submit)
                .width(80);
            rows = rows.push(labelled("ST_DLG_COLUMN_LENGTH", length.into()));
            rows = rows.push(labelled(
                "ST_DLG_COLUMN_ITEMS",
                picker(item, child(TypeStep::Item)),
            ));
            if let Some(inner) = inner_pickers(item, &child(TypeStep::Item)) {
                rows = rows.push(inner);
            }
        }
        TypeSpec::List(item) => {
            rows = rows.push(labelled(
                "ST_DLG_COLUMN_ITEMS",
                picker(item, child(TypeStep::Item)),
            ));
            if let Some(inner) = inner_pickers(item, &child(TypeStep::Item)) {
                rows = rows.push(inner);
            }
        }
        TypeSpec::Map(key, value) => {
            let at = path.to_vec();
            let keys = pick_list(TypeChoice::ALL, Some(*key), move |k| {
                ColumnMessage::Key(at.clone(), k)
            })
            .width(Length::Fill);
            rows = rows.push(labelled("ST_DLG_COLUMN_KEYS", keys.into()));
            rows = rows.push(labelled(
                "ST_DLG_COLUMN_VALUES",
                picker(value, child(TypeStep::Item)),
            ));
            if let Some(inner) = inner_pickers(value, &child(TypeStep::Item)) {
                rows = rows.push(inner);
            }
        }
        TypeSpec::Struct(fields) => {
            rows = rows.push(text(tr("ST_DLG_COLUMN_FIELDS")));
            for (i, (name, field)) in fields.iter().enumerate() {
                let (at, again) = (path.to_vec(), path.to_vec());
                rows = rows.push(
                    row![
                        text_input(&tr("ST_DLG_COLUMN_FIELD_NAME"), name)
                            .on_input(move |n| ColumnMessage::FieldName(at.clone(), i, n))
                            .on_submit(ColumnMessage::Submit)
                            .width(Length::FillPortion(2)),
                        iced::widget::container(picker(field, child(TypeStep::Field(i))))
                            .width(Length::FillPortion(3)),
                        button(text("✕"))
                            .style(button::text)
                            .on_press(ColumnMessage::RemoveField(again, i)),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                );
                if let Some(inner) = inner_pickers(field, &child(TypeStep::Field(i))) {
                    rows = rows.push(inner);
                }
            }
            rows = rows.push(
                button(text(tr("ST_DLG_COLUMN_ADD_FIELD")))
                    .style(button::secondary)
                    .on_press(ColumnMessage::AddField(path.to_vec())),
            );
        }
    }
    Some(
        iced::widget::container(rows)
            .padding(iced::Padding::ZERO.left(16))
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_rename_build_commands() {
        let mut add = ColumnDialog::add(2);
        add.update(ColumnMessage::Name(" total ".into()));
        add.update(ColumnMessage::Type(
            vec![],
            TypeOption::Plain(TypeChoice::Decimal),
        ));
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
        map.update(ColumnMessage::Type(vec![], TypeOption::Map));
        map.update(ColumnMessage::Key(vec![], TypeChoice::Date));
        map.update(ColumnMessage::Type(
            vec![TypeStep::Item],
            TypeOption::Plain(TypeChoice::Integer),
        ));
        let scores = veta_core::display::map_type(DataType::Date32, DataType::Int64);
        assert_eq!(
            map.update(ColumnMessage::Submit),
            Some(Command::AddColumn {
                name: "scores".into(),
                data_type: scores.clone(),
                at: 0
            })
        );
        let mut xyz = ColumnDialog::add(0);
        xyz.update(ColumnMessage::Name("xyz".into()));
        xyz.update(ColumnMessage::Type(vec![], TypeOption::FixedList));
        xyz.update(ColumnMessage::Length(vec![], "x".into()));
        assert_eq!(xyz.update(ColumnMessage::Submit), None, "not a length");
        xyz.update(ColumnMessage::Length(vec![], "2".into()));
        let Some(Command::AddColumn { data_type, .. }) = xyz.update(ColumnMessage::Submit) else {
            panic!("a command");
        };
        assert_eq!(
            veta_core::display::type_name(&data_type),
            "fixed_size_list<string, 2>"
        );
        assert_eq!(ColumnDialog::add(0).with_type(&data_type).spec, xyz.spec);
        let again = ColumnDialog::add(0).with_type(&scores);
        assert_eq!(
            again.spec,
            TypeSpec::Map(
                TypeChoice::Date,
                Box::new(TypeSpec::Plain(TypeChoice::Integer))
            )
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
