//! [`Node`]: one value inside an Arrow array, and its children. Structs,
//! lists and maps all have children, which are nodes too, so a value nested
//! to any depth is walked the same way. Nodes point into the array; nothing
//! is copied.

use std::ops::Range;

use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::DataType;
use arrow::util::display::{ArrayFormatter, FormatOptions};

use crate::display::{PathPart, items_of};

/// What kind of value a node holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A plain value (number, text, date, …).
    Value,
    Struct,
    /// A list (also a fixed-size list).
    List,
    Map,
}

impl NodeKind {
    /// The kind of values of `data_type`.
    pub fn of(data_type: &DataType) -> Self {
        match data_type {
            DataType::Struct(_) => Self::Struct,
            DataType::List(_) | DataType::LargeList(_) | DataType::FixedSizeList(..) => Self::List,
            DataType::Map(..) => Self::Map,
            _ => Self::Value,
        }
    }

    /// Whether values of this kind have children.
    pub fn is_container(self) -> bool {
        self != Self::Value
    }
}

/// One value: item `index` of `array`.
#[derive(Debug, Clone)]
pub struct Node {
    array: ArrayRef,
    index: usize,
    /// Null, also when a parent struct is null.
    null: bool,
}

/// A child of a node: the step to it, its label (field name, index or map
/// key, as text) and the child itself.
#[derive(Debug, Clone)]
pub struct Child {
    pub part: PathPart,
    pub label: String,
    pub node: Node,
}

impl Node {
    /// Value `index` of `array`.
    pub fn new(array: ArrayRef, index: usize) -> Self {
        let null = array.is_null(index);
        Self { array, index, null }
    }

    pub fn data_type(&self) -> &DataType {
        self.array.data_type()
    }

    pub fn kind(&self) -> NodeKind {
        NodeKind::of(self.data_type())
    }

    pub fn is_null(&self) -> bool {
        self.null
    }

    /// A fixed-size list: its items can be set but not added or removed.
    pub fn is_fixed_size(&self) -> bool {
        matches!(self.data_type(), DataType::FixedSizeList(..))
    }

    /// Number of children: a struct's fields or a fixed-size list's items
    /// (also when it is null, as null values), a list's items or a map's
    /// entries (none when null).
    pub fn len(&self) -> usize {
        match self.data_type() {
            DataType::Struct(fields) => fields.len(),
            DataType::FixedSizeList(_, n) => *n as usize,
            _ => self.items().map_or(0, |(range, _, _)| range.len()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Child `i`, if there is one.
    pub fn child(&self, i: usize) -> Option<Child> {
        if let DataType::Struct(fields) = self.data_type() {
            let field = fields.get(i)?;
            let column = self.array.as_struct().column(i).clone();
            let null = self.null || column.is_null(self.index);
            return Some(Child {
                part: PathPart::Field(field.name().clone()),
                label: field.name().clone(),
                node: Node {
                    array: column,
                    index: self.index,
                    null,
                },
            });
        }
        // A null fixed-size list still has its items, as nulls.
        if let DataType::FixedSizeList(_, n) = self.data_type()
            && self.null
        {
            let n = *n as usize;
            let values = self.array.as_fixed_size_list().values().clone();
            return (i < n).then(|| Child {
                part: PathPart::Index(i),
                label: i.to_string(),
                node: Node {
                    array: values,
                    index: self.index * n + i,
                    null: true,
                },
            });
        }
        let (range, values, keys) = self.items()?;
        let at = range.clone().nth(i)?;
        let node = Node::new(values, at);
        Some(match keys {
            Some(keys) => Child {
                part: PathPart::Entry(i),
                label: format_value(&keys, at).unwrap_or_default(),
                node,
            },
            None => Child {
                part: PathPart::Index(i),
                label: i.to_string(),
                node,
            },
        })
    }

    /// The children, in order.
    pub fn children(&self) -> impl Iterator<Item = Child> + '_ {
        (0..self.len()).map_while(|i| self.child(i))
    }

    /// The node at `path` below this one.
    pub fn get(&self, path: &[PathPart]) -> Option<Node> {
        let mut node = self.clone();
        for part in path {
            let i = match (part, node.data_type()) {
                (PathPart::Field(name), DataType::Struct(fields)) => {
                    fields.iter().position(|f| f.name() == name)?
                }
                (PathPart::Index(i), _) if node.kind() == NodeKind::List => *i,
                (PathPart::Entry(i), DataType::Map(..)) => *i,
                _ => return None,
            };
            node = node.child(i)?.node;
        }
        Some(node)
    }

    /// The value as text, e.g. `42`, `{x: 1, y: 2}`, `[a, b]`; `None` for
    /// null.
    pub fn text(&self) -> Option<String> {
        if self.null {
            return None;
        }
        format_value(&self.array, self.index)
    }

    /// Item range, values and (for maps) keys of a list or map value.
    fn items(&self) -> Option<(Range<usize>, ArrayRef, Option<ArrayRef>)> {
        if self.null {
            return None;
        }
        items_of(self.array.as_ref(), self.index)
    }
}

/// Value `index` of `array` as displayed in the grid; `None` for null.
fn format_value(array: &ArrayRef, index: usize) -> Option<String> {
    if array.is_null(index) {
        return None;
    }
    let options = FormatOptions::default().with_null("null");
    let formatter = ArrayFormatter::try_new(array.as_ref(), &options).ok()?;
    Some(formatter.value(index).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{map_type, type_at};
    use crate::values::parse_value;

    fn node(json: &str, data_type: &DataType) -> Node {
        Node::new(parse_value(Some(json), data_type).unwrap(), 0)
    }

    #[test]
    fn walks_values_nested_to_any_depth() {
        let schema = veta_testkit::fixtures::deep_nested_schema();
        let orders = schema.field_with_name("orders").unwrap().data_type();
        let value = node(
            r#"[{"sku": "A", "qty": 2, "price": 1.5, "options": {"size": "M", "color": "red"}},
                null]"#,
            orders,
        );
        assert_eq!(value.kind(), NodeKind::List);
        assert_eq!(value.len(), 2);
        let labels: Vec<String> = value.children().map(|c| c.label).collect();
        assert_eq!(labels, ["0", "1"]);
        let path = [
            PathPart::Index(0),
            PathPart::Field("options".into()),
            PathPart::Entry(1),
        ];
        let color = value.get(&path).unwrap();
        assert_eq!(color.text().as_deref(), Some("red"));
        assert_eq!(type_at(orders, &path), Some(&DataType::Utf8));
        let options = value.get(&path[..2]).unwrap();
        assert_eq!(options.kind(), NodeKind::Map);
        let keys: Vec<String> = options.children().map(|c| c.label).collect();
        assert_eq!(keys, ["size", "color"]);
        // A null struct still lists its fields, as nulls.
        let second = value.get(&[PathPart::Index(1)]).unwrap();
        assert!(second.is_null());
        assert_eq!(second.len(), 4);
        let sku = second.child(0).unwrap();
        assert_eq!((sku.label.as_str(), sku.node.is_null()), ("sku", true));
        assert!(
            second
                .get(&[PathPart::Field("options".into())])
                .unwrap()
                .is_empty()
        );
        assert!(value.get(&[PathPart::Index(2)]).is_none());
        assert!(value.get(&[PathPart::Entry(0)]).is_none());

        // Lists of lists, and maps with whole-number keys.
        let matrix = node(
            "[[1, 2], [], null]",
            &DataType::new_list(DataType::new_list(DataType::Int64, true), true),
        );
        let rows: Vec<(bool, usize)> = matrix
            .children()
            .map(|c| (c.node.is_null(), c.node.len()))
            .collect();
        assert_eq!(rows, [(false, 2), (false, 0), (true, 0)]);
        assert_eq!(
            matrix
                .get(&[PathPart::Index(0), PathPart::Index(1)])
                .unwrap()
                .text()
                .as_deref(),
            Some("2")
        );
        // A null fixed-size list opens into its items, as nulls.
        let xyz = DataType::FixedSizeList(
            std::sync::Arc::new(arrow::datatypes::Field::new("item", DataType::Int64, true)),
            3,
        );
        let null = node("null", &xyz);
        assert_eq!((null.len(), null.is_fixed_size()), (3, true));
        assert!(null.child(2).unwrap().node.is_null());
        assert!(null.child(3).is_none());
        let set = node("[1, 2, 3]", &xyz);
        assert_eq!(set.child(2).unwrap().node.text().as_deref(), Some("3"));
        let by_id = node(r#"{"7": "x"}"#, &map_type(DataType::Int64, DataType::Utf8));
        let entry = by_id.child(0).unwrap();
        assert_eq!(
            (entry.label.as_str(), entry.part),
            ("7", PathPart::Entry(0))
        );
    }
}
