use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, container, scrollable, text, Column, Space};
use iced::{Border, Element, Length, Padding};

use crate::theme::{colors, icons};
use artemis_core::protocol::ServerPayload;

#[derive(Debug, Clone)]
pub enum ServerListMsg {
    GoHome,
    SelectServer(usize),
}

/// Artemis logo / home color (distinct from server accent)
const HOME_COLOR: iced::Color = iced::Color::from_rgb(
    0x5B as f32 / 255.0,
    0x6E as f32 / 255.0,
    0xAE as f32 / 255.0,
);

fn home_button<'a>(is_home: bool) -> Element<'a, ServerListMsg> {
    let label = text(icons::HOME) // nf-fa-home
        .size(22)
        .color(colors::TEXT_PRIMARY)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center);

    // Nerd Font monospace glyphs have slight right-side bearing;
    // compensate with asymmetric padding: [top, right, bottom, left]
    let btn = button(
        container(label)
            .width(48)
            .height(48)
            .padding(Padding {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 2.0,
            })
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center),
    )
    .on_press(ServerListMsg::GoHome)
    .padding(0)
    .style(move |_theme, status| {
        let bg = if is_home {
            HOME_COLOR
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                _ => colors::BG_DARK,
            }
        };
        let radius = if is_home { 14.0 } else { 24.0 };
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

    container(btn).align_x(Horizontal::Center).width(72).into()
}

fn separator<'a>() -> Element<'a, ServerListMsg> {
    container(
        container(Space::with_height(0))
            .width(40)
            .height(2)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(colors::BG_HOVER)),
                border: Border {
                    radius: 1.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
    )
    .width(72)
    .align_x(Horizontal::Center)
    .padding(Padding::from([4, 0]))
    .into()
}

fn server_button<'a>(
    label_text: String,
    index: usize,
    is_active: bool,
) -> Element<'a, ServerListMsg> {
    let label = text(label_text)
        .size(18)
        .color(colors::TEXT_PRIMARY)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center);

    let btn = button(
        container(label)
            .width(48)
            .height(48)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center),
    )
    .on_press(ServerListMsg::SelectServer(index))
    .padding(0)
    .style(move |_theme, status| {
        let bg = if is_active {
            colors::ACCENT
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                _ => colors::BG_DARK,
            }
        };
        let radius = if is_active { 14.0 } else { 24.0 };
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

    container(btn).align_x(Horizontal::Center).width(72).into()
}

fn add_server_button<'a>() -> Element<'a, ServerListMsg> {
    let btn = button(
        container(
            text(icons::PLUS)
                .size(20)
                .color(colors::STATUS_ONLINE)
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .width(48)
        .height(48)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center),
    )
    .padding(0)
    .style(|_theme, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
            _ => colors::BG_DARK,
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: colors::STATUS_ONLINE,
            border: Border {
                radius: 24.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    });

    container(btn).align_x(Horizontal::Center).width(72).into()
}

pub fn view_from_payloads<'a>(
    servers: &'a [ServerPayload],
    active_idx: Option<usize>,
    is_home: bool,
) -> Element<'a, ServerListMsg> {
    let mut items = Column::new().spacing(8).padding(Padding::from([8, 0]));

    // Artemis logo (home button)
    items = items.push(home_button(is_home));

    // Separator
    items = items.push(separator());

    // Server icons
    for (i, server) in servers.iter().enumerate() {
        let is_active = !is_home && active_idx == Some(i);
        let initial = server
            .name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        items = items.push(server_button(initial, i, is_active));
    }

    // Add server button
    items = items.push(add_server_button());

    let content = scrollable(items).height(Length::Fill);

    container(content)
        .width(72)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(colors::BG_DARKEST)),
            ..container::Style::default()
        })
        .padding(Padding::from([4, 0]))
        .into()
}
