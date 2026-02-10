use iced::alignment::Vertical;
use iced::widget::{
    button, column, container, row, scrollable, text, text_input, Column, Space,
};
use iced::{Border, Element, Length, Padding};
use uuid::Uuid;

use artemis_core::protocol::ChannelPayload;
use crate::theme::colors;

#[derive(Debug, Clone)]
pub enum ChatAreaMsg {
    InputChanged(String),
    SendMessage,
    LoadOlderMessages,
}

pub fn view<'a>(
    active_channel: Option<&'a artemis_core::Channel>,
    messages: &'a [artemis_core::Message],
    input_value: &str,
    active_channel_id: Option<Uuid>,
) -> Element<'a, ChatAreaMsg> {
    let channel_name = active_channel
        .map(|c| c.name.as_str())
        .unwrap_or("no-channel");

    let channel_topic = active_channel.and_then(|c| c.topic.as_deref());

    // ── Top bar ──
    let mut top_row = row![
        text("#").size(18).color(colors::TEXT_MUTED),
        Space::with_width(6),
        text(channel_name)
            .size(16)
            .color(colors::TEXT_PRIMARY),
    ]
    .spacing(0)
    .align_y(Vertical::Center);

    if let Some(topic) = channel_topic {
        top_row = top_row
            .push(Space::with_width(12))
            .push(
                text("|")
                    .size(14)
                    .color(colors::TEXT_TIMESTAMP),
            )
            .push(Space::with_width(12))
            .push(
                text(topic)
                    .size(13)
                    .color(colors::TEXT_MUTED),
            );
    }

    let top_bar = container(top_row)
        .padding(Padding::from([12, 16]))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_MEDIUM)),
            border: Border {
                color: colors::BG_DARKEST,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

    // ── Messages ──
    let filtered: Vec<&artemis_core::Message> = messages
        .iter()
        .filter(|m| {
            active_channel_id
                .map(|id| m.channel_id == id)
                .unwrap_or(false)
        })
        .collect();

    let mut msg_column = Column::new().spacing(4).padding(Padding::from([8, 16]));

    let mut prev_author: Option<&Uuid> = None;

    for msg in &filtered {
        let is_continuation =
            prev_author.map(|id| id == &msg.author_id).unwrap_or(false);

        let timestamp: String = msg.timestamp.format("%H:%M").to_string();

        if is_continuation {
            // Compact message (same author)
            let msg_row = row![
                Space::with_width(48), // avatar placeholder space
                text(&msg.content)
                    .size(14)
                    .color(colors::TEXT_PRIMARY),
            ];
            msg_column = msg_column.push(msg_row);
        } else {
            // Full message with avatar + name
            if prev_author.is_some() {
                msg_column = msg_column.push(Space::with_height(8));
            }

            // Avatar circle placeholder
            let avatar_initial: String = msg
                .author_name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();

            let avatar = container(
                text(avatar_initial)
                    .size(14)
                    .color(colors::TEXT_PRIMARY)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .width(36)
            .height(36)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(Vertical::Center)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(colors::ACCENT)),
                border: Border {
                    radius: 18.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            });

            let header = row![
                text(&msg.author_name)
                    .size(14)
                    .color(colors::ROLE_FOUNDER),
                Space::with_width(8),
                text(timestamp)
                    .size(11)
                    .color(colors::TEXT_TIMESTAMP),
            ]
            .align_y(Vertical::Center);

            let body = text(&msg.content)
                .size(14)
                .color(colors::TEXT_PRIMARY);

            let msg_content = column![header, body].spacing(2);

            let msg_row = row![avatar, Space::with_width(12), msg_content]
                .align_y(Vertical::Top);

            msg_column = msg_column.push(msg_row);
        }

        prev_author = Some(&msg.author_id);
    }

    if filtered.is_empty() {
        msg_column = msg_column.push(
            container(
                text(format!("Welcome to #{}! No messages yet.", channel_name))
                    .size(14)
                    .color(colors::TEXT_MUTED),
            )
            .padding(20),
        );
    }

    let messages_area = scrollable(msg_column)
        .height(Length::Fill)
        .width(Length::Fill);

    // ── Input bar ──
    let placeholder = format!("Message #{}", channel_name);

    let input = text_input(&placeholder, input_value)
        .on_input(ChatAreaMsg::InputChanged)
        .on_submit(ChatAreaMsg::SendMessage)
        .padding(Padding::from([10, 12]))
        .size(14);

    let send_btn = button(text("Send").size(13).color(colors::TEXT_PRIMARY))
        .on_press(ChatAreaMsg::SendMessage)
        .padding(Padding::from([8, 16]))
        .style(|_theme, _status| button::Style {
            background: Some(iced::Background::Color(colors::ACCENT)),
            text_color: colors::TEXT_PRIMARY,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        });

    let input_bar = container(
        row![
            input,
            Space::with_width(8),
            send_btn,
        ]
        .align_y(Vertical::Center),
    )
    .padding(Padding::from([8, 16]))
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(colors::BG_DARK)),
        ..container::Style::default()
    });

    // ── Assemble ──
    let chat_column = column![top_bar, messages_area, input_bar];

    container(chat_column)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_MEDIUM)),
            ..container::Style::default()
        })
        .into()
}

pub fn view_with_payload<'a>(
    active_channel: Option<&'a ChannelPayload>,
    messages: &'a [artemis_core::Message],
    input_value: &str,
    active_channel_id: Option<Uuid>,
) -> Element<'a, ChatAreaMsg> {
    let channel_name = active_channel
        .map(|c| c.name.as_str())
        .unwrap_or("no-channel");

    let channel_topic = active_channel.and_then(|c| c.topic.as_deref());

    // ── Top bar ──
    let mut top_row = row![
        text("#").size(18).color(colors::TEXT_MUTED),
        Space::with_width(6),
        text(channel_name).size(16).color(colors::TEXT_PRIMARY),
    ]
    .spacing(0)
    .align_y(Vertical::Center);

    if let Some(topic) = channel_topic {
        top_row = top_row
            .push(Space::with_width(12))
            .push(text("|").size(14).color(colors::TEXT_TIMESTAMP))
            .push(Space::with_width(12))
            .push(text(topic).size(13).color(colors::TEXT_MUTED));
    }

    let top_bar = container(top_row)
        .padding(Padding::from([12, 16]))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_MEDIUM)),
            border: Border {
                color: colors::BG_DARKEST,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

    // ── Messages ──
    let filtered: Vec<&artemis_core::Message> = messages
        .iter()
        .filter(|m| {
            active_channel_id
                .map(|id| m.channel_id == id)
                .unwrap_or(false)
        })
        .collect();

    let mut msg_column = Column::new().spacing(4).padding(Padding::from([8, 16]));

    // Load older messages button
    if !filtered.is_empty() {
        let load_btn = button(
            container(
                text("Load older messages")
                    .size(12)
                    .color(colors::TEXT_MUTED)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center),
        )
        .on_press(ChatAreaMsg::LoadOlderMessages)
        .width(Length::Fill)
        .style(|_theme, status| {
            let bg = match status {
                button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                _ => colors::BG_MEDIUM,
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: colors::TEXT_MUTED,
                border: Border {
                    radius: 4.0.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        });
        msg_column = msg_column.push(load_btn);
        msg_column = msg_column.push(Space::with_height(8));
    }

    let mut prev_author: Option<&Uuid> = None;

    for msg in &filtered {
        let is_continuation =
            prev_author.map(|id| id == &msg.author_id).unwrap_or(false);

        let timestamp: String = msg.timestamp.format("%H:%M").to_string();

        if is_continuation {
            let msg_row = row![
                Space::with_width(48),
                text(&msg.content).size(14).color(colors::TEXT_PRIMARY),
            ];
            msg_column = msg_column.push(msg_row);
        } else {
            if prev_author.is_some() {
                msg_column = msg_column.push(Space::with_height(8));
            }

            let avatar_initial: String = msg
                .author_name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();

            let avatar = container(
                text(avatar_initial)
                    .size(14)
                    .color(colors::TEXT_PRIMARY)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .width(36)
            .height(36)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(Vertical::Center)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(colors::ACCENT)),
                border: Border {
                    radius: 18.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            });

            let header = row![
                text(&msg.author_name).size(14).color(colors::ROLE_FOUNDER),
                Space::with_width(8),
                text(timestamp).size(11).color(colors::TEXT_TIMESTAMP),
            ]
            .align_y(Vertical::Center);

            let body = text(&msg.content).size(14).color(colors::TEXT_PRIMARY);

            let msg_content = column![header, body].spacing(2);

            let msg_row = row![avatar, Space::with_width(12), msg_content]
                .align_y(Vertical::Top);

            msg_column = msg_column.push(msg_row);
        }

        prev_author = Some(&msg.author_id);
    }

    if filtered.is_empty() {
        msg_column = msg_column.push(
            container(
                text(format!("Welcome to #{}! No messages yet.", channel_name))
                    .size(14)
                    .color(colors::TEXT_MUTED),
            )
            .padding(20),
        );
    }

    let messages_area = scrollable(msg_column)
        .height(Length::Fill)
        .width(Length::Fill);

    // ── Input bar ──
    let placeholder = format!("Message #{}", channel_name);

    let input = text_input(&placeholder, input_value)
        .on_input(ChatAreaMsg::InputChanged)
        .on_submit(ChatAreaMsg::SendMessage)
        .padding(Padding::from([10, 12]))
        .size(14);

    let send_btn = button(text("Send").size(13).color(colors::TEXT_PRIMARY))
        .on_press(ChatAreaMsg::SendMessage)
        .padding(Padding::from([8, 16]))
        .style(|_theme, _status| button::Style {
            background: Some(iced::Background::Color(colors::ACCENT)),
            text_color: colors::TEXT_PRIMARY,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        });

    let input_bar = container(
        row![input, Space::with_width(8), send_btn,].align_y(Vertical::Center),
    )
    .padding(Padding::from([8, 16]))
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(colors::BG_DARK)),
        ..container::Style::default()
    });

    let chat_column = column![top_bar, messages_area, input_bar];

    container(chat_column)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_MEDIUM)),
            ..container::Style::default()
        })
        .into()
}
