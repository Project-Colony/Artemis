use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, container, scrollable, text, Column};
use iced::{Border, Element, Length, Padding};

use artemis_core::protocol::ServerPayload;
use crate::theme::colors;

#[derive(Debug, Clone)]
pub enum ServerListMsg {
    SelectServer(usize),
}

pub fn view<'a>(
    servers: &'a [artemis_core::Server],
    active_idx: usize,
) -> Element<'a, ServerListMsg> {
    let mut items = Column::new().spacing(8).padding(Padding::from([8, 0]));

    for (i, server) in servers.iter().enumerate() {
        let is_active = i == active_idx;

        let initial = server
            .name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();

        let label = text(initial)
            .size(16)
            .color(colors::TEXT_PRIMARY)
            .align_x(Horizontal::Center);

        let btn = button(
            container(label)
                .width(40)
                .height(40)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .on_press(ServerListMsg::SelectServer(i))
        .style(move |_theme, status| {
            let bg = if is_active {
                colors::ACCENT
            } else {
                match status {
                    button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                    _ => colors::BG_DARK,
                }
            };
            let radius = if is_active { 12.0 } else { 20.0 };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: colors::TEXT_PRIMARY,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        });

        items = items.push(container(btn).align_x(Horizontal::Center).width(56));
    }

    // Add server button
    let add_btn = button(
        container(
            text("+").size(20).color(colors::STATUS_ONLINE).align_x(Horizontal::Center),
        )
        .width(40)
        .height(40)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center),
    )
    .style(|_theme, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
            _ => colors::BG_DARK,
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: colors::STATUS_ONLINE,
            border: Border {
                radius: 20.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    });

    items = items.push(container(add_btn).align_x(Horizontal::Center).width(56));

    let content = scrollable(items).height(Length::Fill);

    container(content)
        .width(56)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(colors::BG_DARKEST)),
            ..container::Style::default()
        })
        .padding(Padding::from([4, 0]))
        .into()
}

pub fn view_from_payloads<'a>(
    servers: &'a [ServerPayload],
    active_idx: usize,
) -> Element<'a, ServerListMsg> {
    let mut items = Column::new().spacing(8).padding(Padding::from([8, 0]));

    for (i, server) in servers.iter().enumerate() {
        let is_active = i == active_idx;

        let initial = server
            .name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();

        let label = text(initial)
            .size(16)
            .color(colors::TEXT_PRIMARY)
            .align_x(Horizontal::Center);

        let btn = button(
            container(label)
                .width(40)
                .height(40)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .on_press(ServerListMsg::SelectServer(i))
        .style(move |_theme, status| {
            let bg = if is_active {
                colors::ACCENT
            } else {
                match status {
                    button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                    _ => colors::BG_DARK,
                }
            };
            let radius = if is_active { 12.0 } else { 20.0 };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: colors::TEXT_PRIMARY,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        });

        items = items.push(container(btn).align_x(Horizontal::Center).width(56));
    }

    let add_btn = button(
        container(
            text("+").size(20).color(colors::STATUS_ONLINE).align_x(Horizontal::Center),
        )
        .width(40)
        .height(40)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center),
    )
    .style(|_theme, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
            _ => colors::BG_DARK,
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: colors::STATUS_ONLINE,
            border: Border {
                radius: 20.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    });

    items = items.push(container(add_btn).align_x(Horizontal::Center).width(56));

    let content = scrollable(items).height(Length::Fill);

    container(content)
        .width(56)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(colors::BG_DARKEST)),
            ..container::Style::default()
        })
        .padding(Padding::from([4, 0]))
        .into()
}
