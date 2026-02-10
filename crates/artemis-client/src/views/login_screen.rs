use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, text, text_input, Space};
use iced::{Border, Element, Length, Padding};

use crate::theme::colors;

#[derive(Debug, Clone)]
pub enum LoginMsg {
    ServerUrlChanged(String),
    TokenChanged(String),
    Connect,
    UseMockData,
}

pub fn view<'a>(
    server_url: &'a str,
    token: &'a str,
    error: Option<&'a str>,
    connecting: bool,
) -> Element<'a, LoginMsg> {
    let title = text("Artemis")
        .size(40)
        .color(colors::ACCENT);

    let subtitle = text("Connect to your server")
        .size(14)
        .color(colors::TEXT_MUTED);

    let server_input = text_input("Server URL (e.g. http://localhost:3000)", server_url)
        .on_input(LoginMsg::ServerUrlChanged)
        .padding(Padding::from([10, 12]))
        .size(14);

    let token_input = text_input("Auth token (from GitHub OAuth)", token)
        .on_input(LoginMsg::TokenChanged)
        .padding(Padding::from([10, 12]))
        .size(14);

    let connect_label = if connecting { "Connecting..." } else { "Connect" };

    let mut connect_btn = button(
        container(
            text(connect_label)
                .size(14)
                .color(colors::TEXT_PRIMARY)
                .align_x(Horizontal::Center),
        )
        .width(Length::Fill)
        .align_x(Horizontal::Center),
    )
    .padding(Padding::from([10, 20]))
    .width(Length::Fill)
    .style(|_theme, _status| button::Style {
        background: Some(iced::Background::Color(colors::ACCENT)),
        text_color: colors::TEXT_PRIMARY,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    });

    if !connecting {
        connect_btn = connect_btn.on_press(LoginMsg::Connect);
    }

    let mock_btn = button(
        container(
            text("Demo Mode (no server)")
                .size(13)
                .color(colors::TEXT_MUTED)
                .align_x(Horizontal::Center),
        )
        .width(Length::Fill)
        .align_x(Horizontal::Center),
    )
    .on_press(LoginMsg::UseMockData)
    .padding(Padding::from([8, 16]))
    .width(Length::Fill)
    .style(|_theme, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
            _ => colors::BG_INPUT,
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: colors::TEXT_MUTED,
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    });

    let mut form = column![
        title,
        Space::with_height(4),
        subtitle,
        Space::with_height(24),
        text("Server URL").size(12).color(colors::TEXT_MUTED),
        Space::with_height(4),
        server_input,
        Space::with_height(16),
        text("Auth Token").size(12).color(colors::TEXT_MUTED),
        Space::with_height(4),
        token_input,
        Space::with_height(24),
        connect_btn,
        Space::with_height(8),
        mock_btn,
    ]
    .align_x(Horizontal::Center)
    .width(360);

    if let Some(err) = error {
        form = form.push(Space::with_height(12));
        form = form.push(
            text(err)
                .size(13)
                .color(iced::color!(0xEF, 0x44, 0x44)),
        );
    }

    let hint = text("Go to /auth/github on your server to get a token")
        .size(11)
        .color(colors::TEXT_TIMESTAMP);

    form = form.push(Space::with_height(16));
    form = form.push(hint);

    container(
        container(form)
            .padding(40)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(colors::BG_DARK)),
                border: Border {
                    radius: 12.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(colors::BG_DARKEST)),
        ..container::Style::default()
    })
    .into()
}
