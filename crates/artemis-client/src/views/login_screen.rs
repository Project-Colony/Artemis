use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, text, Space};
use iced::{Border, Element, Length, Padding};

use crate::net::GITHUB_DEVICE_PAGE;
use crate::theme::{colors, icons};

#[derive(Debug, Clone)]
pub enum LoginMsg {
    SignInWithGitHub,
    UseMockData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginState {
    Idle,
    WaitingForCode { user_code: String },
    Polling,
    Initializing,
    Error(String),
}

pub fn view(state: &LoginState) -> Element<'_, LoginMsg> {
    let title = text(format!("{} Artemis", icons::CHAT))
        .size(40)
        .color(colors::ACCENT);

    let subtitle = text(format!(
        "{} Chat through an Artemis relay, signed in with GitHub",
        icons::GLOBE
    ))
    .size(14)
    .color(colors::TEXT_MUTED);

    let mut form = column![
        title,
        Space::new().height(4),
        subtitle,
        Space::new().height(32),
    ]
    .align_x(Horizontal::Center)
    .width(400);

    match state {
        LoginState::Idle => {
            let github_btn = button(
                container(
                    text("\u{f09b}  Sign in with GitHub") // nf-fa-github
                        .size(15)
                        .color(colors::TEXT_PRIMARY)
                        .align_x(Horizontal::Center),
                )
                .width(Length::Fill)
                .align_x(Horizontal::Center),
            )
            .on_press(LoginMsg::SignInWithGitHub)
            .padding(Padding::from([12, 24]))
            .width(Length::Fill)
            .style(|_theme, status| {
                let bg = match status {
                    button::Status::Hovered | button::Status::Pressed => {
                        iced::Color::from_rgb(0.15, 0.15, 0.15)
                    }
                    _ => iced::Color::from_rgb(0.1, 0.1, 0.1),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: colors::TEXT_PRIMARY,
                    border: Border {
                        radius: 8.0.into(),
                        width: 1.0,
                        color: iced::Color::from_rgb(0.3, 0.3, 0.3),
                    },
                    ..button::Style::default()
                }
            });

            form = form.push(github_btn);
            form = form.push(Space::new().height(12));

            let mock_btn = button(
                container(
                    text("Demo Mode (no connection)")
                        .size(13)
                        .color(colors::TEXT_MUTED)
                        .align_x(Horizontal::Center),
                )
                .width(Length::Fill)
                .align_x(Horizontal::Center),
            )
            .on_press(LoginMsg::UseMockData)
            .padding(Padding::from([10, 20]))
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

            form = form.push(mock_btn);
        }

        LoginState::WaitingForCode { user_code } => {
            form = form.push(
                text("Enter this code on GitHub:")
                    .size(14)
                    .color(colors::TEXT_MUTED),
            );
            form = form.push(Space::new().height(16));
            form = form.push(
                container(
                    text(user_code)
                        .size(32)
                        .color(colors::TEXT_PRIMARY)
                        .align_x(Horizontal::Center),
                )
                .width(Length::Fill)
                .padding(Padding::from([16, 24]))
                .align_x(Horizontal::Center)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(colors::BG_INPUT)),
                    border: Border {
                        radius: 8.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
            );
            form = form.push(Space::new().height(12));
            form = form.push(
                text(format!("{} Go to: {}", icons::GLOBE, GITHUB_DEVICE_PAGE))
                    .size(12)
                    .color(colors::ACCENT),
            );
            form = form.push(Space::new().height(8));
            form = form.push(
                text("Your browser should open automatically...")
                    .size(11)
                    .color(colors::TEXT_TIMESTAMP),
            );
            form = form.push(Space::new().height(16));
            form = form.push(
                text("Waiting for authorization...")
                    .size(13)
                    .color(colors::TEXT_MUTED),
            );
        }

        LoginState::Polling => {
            form = form.push(
                text("Asking the relay for a sign-in code...")
                    .size(14)
                    .color(colors::TEXT_MUTED),
            );
        }

        LoginState::Initializing => {
            form = form.push(
                text(format!("{} Connecting to the relay...", icons::LOCK))
                    .size(14)
                    .color(colors::ACCENT),
            );
        }

        LoginState::Error(err) => {
            form = form.push(text(err).size(13).color(iced::color!(0xEF, 0x44, 0x44)));
            form = form.push(Space::new().height(16));

            let retry_btn = button(
                container(
                    text("Try Again")
                        .size(14)
                        .color(colors::TEXT_PRIMARY)
                        .align_x(Horizontal::Center),
                )
                .width(Length::Fill)
                .align_x(Horizontal::Center),
            )
            .on_press(LoginMsg::SignInWithGitHub)
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

            form = form.push(retry_btn);
        }
    }

    container(container(form).padding(40).style(|_| container::Style {
        background: Some(iced::Background::Color(colors::BG_DARK)),
        border: Border {
            radius: 12.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }))
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
