//! The Value tab: the selected cell's value as an expandable tree.

use super::*;
use iced::widget::column;

/// What the Value tab shows: the cell's tree, collapsed nodes and the leaf
/// being edited (pointer, text, error).
pub struct ValueView<'v> {
    pub tree: Option<ValueNode>,
    pub collapsed: &'v HashSet<String>,
    pub leaf_edit: Option<LeafEditView<'v>>,
}

/// The selected cell's value as an expandable tree.
/// Expand/explode entries for a column of `column_type`.
pub(crate) fn nested_items(
    column_type: Option<&DataType>,
) -> Vec<Option<(Icon, String, &'static str, MenuItem)>> {
    match column_type {
        Some(DataType::Struct(fields)) => {
            let mut items = vec![
                Some((
                    Icon::ListTree,
                    "Expand fields to columns".into(),
                    "",
                    MenuItem::ExpandStruct,
                )),
                Some((Icon::Plus, "Add field…".into(), "", MenuItem::AddField)),
            ];
            if fields
                .iter()
                .any(|f| matches!(f.data_type(), DataType::Struct(_)))
            {
                items.push(Some((
                    Icon::ListTree,
                    "Expand all levels".into(),
                    "",
                    MenuItem::ExpandAll,
                )));
            }
            items
        }
        Some(
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(..)
            | DataType::Map(..),
        ) => vec![Some((
            Icon::Rows,
            "Explode to rows".into(),
            "",
            MenuItem::ExplodeList,
        ))],
        _ => Vec::new(),
    }
}

/// The value (or map key) being edited in the Value tab.
#[derive(Debug, Clone, Copy)]
pub struct LeafEditView<'v> {
    pub pointer: &'v [PathPart],
    pub text: &'v str,
    pub error: Option<&'v str>,
    /// Renaming the map key rather than editing the value.
    pub rename: bool,
}

pub(super) fn value_pane<'a>(
    value: Option<ValueNode>,
    collapsed: &HashSet<String>,
    leaf_edit: Option<LeafEditView<'_>>,
    ui: Ui,
) -> Element<'a, Message> {
    let t = ui.tokens;
    let Some(root) = value else {
        return container(
            text("Select a cell to see its value.")
                .size(ui.small())
                .color(t.muted_text),
        )
        .padding([0, 12])
        .into();
    };
    let mut lines = column![
        button(text("Edit as JSON…").size(ui.small()))
            .padding([2, 8])
            .style(move |_theme: &Theme, status| subtle_button(t, status))
            .on_press(Message::OpenJsonEditor)
    ]
    .spacing(2);
    let mut stack: Vec<(ValueNode, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        let has_children = !node.children.is_empty();
        let open = !collapsed.contains(&node.path);
        let marker: Element<'a, Message> = if has_children {
            let glyph = if open { "▾" } else { "▸" };
            button(text(glyph).size(ui.small()).color(t.muted_text))
                .padding(0)
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::ToggleNode(node.path.clone()))
                .into()
        } else {
            iced::widget::Space::new().width(ui.small()).into()
        };
        let value_color = if node.null || has_children {
            t.muted_text
        } else {
            t.text
        };
        // Values inside nested cells can be edited in place.
        let editing = leaf_edit.filter(|e| e.pointer == node.pointer.as_slice());
        let editor = |e: LeafEditView<'_>, placeholder: &str| -> Element<'a, Message> {
            let mut column = column![
                text_input(placeholder, e.text)
                    .id(crate::app::LEAF_ID)
                    .size(ui.small())
                    .padding([1, 4])
                    .on_input(Message::LeafInput)
                    .on_submit(Message::CommitLeaf)
                    .width(140)
            ];
            if let Some(error) = e.error {
                column = column.push(
                    text(error.to_owned())
                        .size(ui.small() - 1.0)
                        .color(Color::from_rgb8(0xd1, 0x43, 0x43)),
                );
            }
            column.into()
        };
        let value: Element<'a, Message> = match editing {
            Some(e) if !e.rename => editor(e, "null"),
            Some(_) => text(node.text.clone())
                .size(ui.small())
                .color(value_color)
                .into(),
            None if !has_children && !node.pointer.is_empty() => {
                let initial = if node.null {
                    String::new()
                } else {
                    node.text.clone()
                };
                button(text(node.text.clone()).size(ui.small()).color(value_color))
                    .padding([0, 3])
                    .style(move |_theme: &Theme, status| subtle_button(t, status))
                    .on_press(Message::EditLeaf(
                        node.pointer.clone(),
                        node.data_type.clone(),
                        initial,
                    ))
                    .into()
            }
            None => text(node.text.clone())
                .size(ui.small())
                .color(value_color)
                .into(),
        };
        // Map keys can be renamed; list items and map entries removed;
        // lists and maps take new items.
        let key = match node.pointer.last() {
            Some(PathPart::Key(key)) => Some(key.clone()),
            _ => None,
        };
        let label: Element<'a, Message> = match (editing, &key) {
            (Some(e), Some(_)) if e.rename => editor(e, "key"),
            (_, Some(key)) => button(text(node.label.clone()).size(ui.small()).font(bold()))
                .padding(0)
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(Message::EditKey(node.pointer.clone(), key.clone()))
                .into(),
            _ => text(node.label.clone())
                .size(ui.small())
                .font(bold())
                .into(),
        };
        let small_button = |glyph: &'static str, message: Message| {
            button(text(glyph).size(ui.small()).color(t.muted_text))
                .padding([0, 4])
                .style(move |_theme: &Theme, status| subtle_button(t, status))
                .on_press(message)
        };
        let add = match node.data_type {
            DataType::List(_) | DataType::LargeList(_) => Some(small_button(
                "+",
                Message::NestedEdit(node.pointer.clone(), NestedEdit::AppendItem),
            )),
            DataType::Map(..) => Some(small_button(
                "+",
                Message::NestedEdit(node.pointer.clone(), NestedEdit::AddEntry),
            )),
            _ => None,
        };
        let removable = matches!(
            node.pointer.last(),
            Some(PathPart::Index(_) | PathPart::Key(_))
        );
        let remove = removable.then(|| {
            small_button(
                "✕",
                Message::NestedEdit(node.pointer.clone(), NestedEdit::Remove),
            )
        });
        lines = lines.push(
            row![
                iced::widget::Space::new().width(depth as f32 * 14.0),
                marker,
                label,
                value,
                // Containers say what they hold; leaves show their type.
                text(if has_children {
                    String::new()
                } else {
                    node.type_name.clone()
                })
                .size(ui.small() - 1.0)
                .color(Color {
                    a: 0.6,
                    ..t.muted_text
                }),
            ]
            .extend(add.map(Into::into))
            .extend(remove.map(Into::into))
            .spacing(6)
            .align_y(Alignment::Center),
        );
        if has_children && open {
            for child in node.children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    scrollable(container(lines).padding([0, 12]))
        .height(Length::Fill)
        .into()
}
