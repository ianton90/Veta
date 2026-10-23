//! Drawing the ribbon: tabs, groups, command buttons with their tooltips
//! and help, and their styles.

use crate::fonts::text;
use iced::widget::{Space, button, column, container, row, rule, tooltip};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use super::layout::{Group, Item, Size, groups};
use super::{Action, Context, Help, Ribbon, RibbonMessage, Tab};
use crate::i18n::tr;
use crate::platform::icon::{Icon, icon};
use crate::settings::theme::Tokens;

const SMALL_PER_COLUMN: usize = 3;

pub fn view<'a, Message: Clone + 'a>(
    ribbon: &Ribbon,
    context: Context,
    on_message: impl Fn(RibbonMessage) -> Message + Copy + 'a,
    on_action: impl Fn(Action) -> Message + Copy + 'a,
) -> Element<'a, Message> {
    let tokens = context.tokens;

    let mut strip = row![].spacing(2).align_y(Alignment::End);
    for tab in Tab::ALL {
        let selected = ribbon.tab == tab && !ribbon.collapsed;
        strip = strip.push(
            button(text(tr(tab.label())).size(context.text_size - 1.0))
                .padding([5, 14])
                .style(move |_theme: &Theme, status| tab_style(tokens, selected, status))
                .on_press(on_message(RibbonMessage::SelectTab(tab))),
        );
    }
    let collapse_icon = if ribbon.collapsed {
        Icon::ChevronDown
    } else {
        Icon::ChevronUp
    };
    strip = strip.push(Space::new().width(Length::Fill)).push(
        tooltip(
            button(icon(collapse_icon, context.text_size))
                .padding([5, 8])
                .style(move |_theme: &Theme, status| item_style(tokens, false, status))
                .on_press(on_message(RibbonMessage::ToggleCollapsed)),
            tip(tr(if ribbon.collapsed {
                "ST_RIBBON_SHOW"
            } else {
                "ST_RIBBON_HIDE"
            })),
            tooltip::Position::Left,
        )
        .padding(6)
        .style(move |_theme: &Theme| tooltip_style(tokens)),
    );

    let mut content = column![container(strip).padding(iced::Padding::from([0, 8]).top(4))];

    if !ribbon.collapsed {
        let mut body = row![].spacing(0).height(context.text_size * 6.6);
        let all = groups(ribbon.tab);
        let count = all.len();
        for (i, g) in all.into_iter().enumerate() {
            body = body.push(group_view(g, context, on_action));
            if i + 1 < count {
                body = body.push(
                    container(rule::vertical(1).style(move |_theme: &Theme| rule_style(tokens)))
                        .padding([8, 0]),
                );
            }
        }
        content = content.push(container(body).width(Length::Fill).padding([4, 8]).style(
            move |_theme: &Theme| {
                container::Style::default()
                    .background(tokens.background)
                    .border(Border {
                        color: tokens.border,
                        width: 0.0,
                        radius: 0.0.into(),
                    })
            },
        ));
    }

    container(content)
        .width(Length::Fill)
        .style(move |_theme: &Theme| container::Style::default().background(tokens.surface))
        .into()
}

fn group_view<'a, Message: Clone + 'a>(
    group: Group,
    context: Context,
    on_action: impl Fn(Action) -> Message + Copy + 'a,
) -> Element<'a, Message> {
    let tokens = context.tokens;
    let mut items = row![].spacing(2).height(Length::Fill);
    let mut smalls: Vec<Item> = Vec::new();

    let flush = |items: iced::widget::Row<'a, Message>, smalls: &mut Vec<Item>| {
        if smalls.is_empty() {
            return items;
        }
        let col = column(
            smalls
                .drain(..)
                .map(|item| item_button(item, context, on_action)),
        )
        .spacing(1);
        items.push(col)
    };

    for item in group.items {
        match item.size {
            Size::Large => {
                items = flush(items, &mut smalls);
                items = items.push(item_button(item, context, on_action));
            }
            Size::Small => {
                smalls.push(item);
                if smalls.len() == SMALL_PER_COLUMN {
                    items = flush(items, &mut smalls);
                }
            }
        }
    }
    items = flush(items, &mut smalls);

    column![
        items,
        text(tr(group.label))
            .size(context.text_size - 3.0)
            .color(tokens.muted_text)
            .wrapping(text::Wrapping::None),
    ]
    .padding([0, 6])
    .spacing(2)
    .align_x(Alignment::Center)
    .into()
}

fn item_button<'a, Message: Clone + 'a>(
    item: Item,
    context: Context,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    let tokens = context.tokens;
    let enabled = context.enabled(item.action);
    let checked = context.checked(item.action);
    let color = if enabled {
        tokens.text
    } else {
        Color {
            a: 0.38,
            ..tokens.text
        }
    };

    let size = context.text_size;
    let content: Element<'a, Message> = match item.size {
        Size::Large => column![
            icon(item.icon, (size * 1.85).round()).color(color),
            text(tr(item.label))
                .size(size - 2.0)
                .color(color)
                .align_x(Alignment::Center)
                .wrapping(text::Wrapping::Word),
        ]
        .spacing(5)
        .width(Length::Shrink)
        .align_x(Alignment::Center)
        .into(),
        Size::Small => row![
            icon(item.icon, size).color(color),
            text(tr(item.label))
                .size(size - 2.0)
                .color(color)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    };

    let padding = match item.size {
        Size::Large => [6, 8],
        Size::Small => [3, 6],
    };
    let mut b = button(content)
        .padding(padding)
        .style(move |_theme: &Theme, status| item_style(tokens, checked, status));
    if let Size::Large = item.size {
        b = b.height(Length::Fill).width(Length::Shrink);
    }
    if enabled {
        b = b.on_press(on_action(item.action));
    }

    let tip_text = match item.action.shortcut() {
        Some(shortcut) => format!("{} ({shortcut})", tr(item.label)),
        None => tr(item.label),
    };
    let content: Element<'a, Message> = match item.action.help() {
        Some(help) => help_view(tip_text, help, tokens),
        None => tip(tip_text).into(),
    };
    tooltip(b, content, tooltip::Position::Bottom)
        .gap(4)
        .padding(6)
        .style(move |_theme: &Theme| tooltip_style(tokens))
        .into()
}

/// A command's title, what it does, and its example: the table before, an
/// arrow and the table after.
fn help_view<'a, Message: 'a>(title: String, help: Help, tokens: Tokens) -> Element<'a, Message> {
    let table = |rows: &'static str| {
        container(text(rows).size(12).font(iced::Font::MONOSPACE))
            .padding([4, 8])
            .style(move |_theme: &Theme| {
                container::Style::default()
                    .background(tokens.surface)
                    .border(Border {
                        color: tokens.border,
                        width: 1.0,
                        radius: 3.0.into(),
                    })
            })
    };
    column![
        tip(title).font(crate::fonts::bold()),
        tip(tr(help.text)).width(300).color(tokens.muted_text),
        row![
            table(help.before),
            tip("→").color(tokens.muted_text),
            table(help.after),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(6)
    .into()
}

fn tip<'a>(content: impl iced::widget::text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(content).size(12)
}

fn tooltip_style(tokens: Tokens) -> container::Style {
    container::Style::default()
        .background(tokens.background)
        .color(tokens.text)
        .border(Border {
            color: tokens.border,
            width: 1.0,
            radius: 4.0.into(),
        })
}

fn rule_style(tokens: Tokens) -> rule::Style {
    rule::Style {
        color: tokens.border,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

fn tab_style(tokens: Tokens, selected: bool, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if selected {
            tokens.background
        } else if hovered {
            Color {
                a: 0.08,
                ..tokens.text
            }
        } else {
            Color::TRANSPARENT
        })),
        text_color: if selected {
            tokens.primary
        } else {
            tokens.text
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: iced::border::Radius::default().top(4.0),
        },
        ..button::Style::default()
    }
}

fn item_style(tokens: Tokens, checked: bool, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Pressed => 0.22,
        button::Status::Hovered => 0.12,
        _ if checked => 0.16,
        _ => 0.0,
    };
    button::Style {
        background: Some(Background::Color(Color {
            a: alpha,
            ..tokens.primary
        })),
        text_color: tokens.text,
        border: Border {
            color: if checked {
                Color {
                    a: 0.5,
                    ..tokens.primary
                }
            } else {
                Color::TRANSPARENT
            },
            width: if checked { 1.0 } else { 0.0 },
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}
