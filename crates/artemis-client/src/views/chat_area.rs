use iced::alignment::Vertical;
use iced::widget::{
    button, column, container, row, scrollable, text, text_input, Column, Row, Space,
};
use iced::{Border, Element, Length, Padding};
use uuid::Uuid;

use artemis_core::models::message::ReactionCount;
use artemis_core::protocol::ChannelPayload;
use crate::theme::{colors, icons};

#[derive(Debug, Clone)]
pub enum ChatAreaMsg {
    InputChanged(String),
    SendMessage,
    LoadOlderMessages,
    /// Start replying to a specific message.
    ReplyTo(Uuid),
    /// Cancel the current reply.
    CancelReply,
    /// Toggle a reaction on a message (emoji string).
    ToggleReaction(Uuid, String),
    /// Pin or unpin a message.
    TogglePin(Uuid, bool),
    /// Search input changed.
    SearchInputChanged(String),
    /// Submit search query.
    SubmitSearch,
}

/// Common quick-reaction emojis.
const QUICK_REACTIONS: &[&str] = &["👍", "❤️", "😂", "🎉", "🔥"];

pub fn view_with_payload<'a>(
    active_channel: Option<&'a ChannelPayload>,
    messages: &'a [artemis_core::Message],
    input_value: &str,
    active_channel_id: Option<Uuid>,
    reply_to: Option<&'a artemis_core::Message>,
    search_query: &str,
) -> Element<'a, ChatAreaMsg> {
    let channel_name = active_channel
        .map(|c| c.name.as_str())
        .unwrap_or("no-channel");

    let channel_topic = active_channel.and_then(|c| c.topic.as_deref());

    // ── Top bar ──
    let mut top_row = row![
        text(icons::CHANNEL_TEXT).size(16).color(colors::TEXT_MUTED),
        Space::with_width(6),
        text(channel_name).size(16).color(colors::TEXT_PRIMARY),
    ]
    .spacing(0)
    .align_y(Vertical::Center);

    if let Some(topic) = channel_topic {
        top_row = top_row
            .push(Space::with_width(12))
            .push(text(icons::DIVIDER).size(14).color(colors::TEXT_TIMESTAMP))
            .push(Space::with_width(12))
            .push(text(topic).size(13).color(colors::TEXT_MUTED));
    }

    // Search bar in top row
    let search_placeholder = format!("{} Search messages...", icons::SEARCH);
    top_row = top_row
        .push(Space::with_width(Length::Fill))
        .push(
            text_input(&search_placeholder, search_query)
                .on_input(ChatAreaMsg::SearchInputChanged)
                .on_submit(ChatAreaMsg::SubmitSearch)
                .padding(Padding::from([4, 8]))
                .size(12)
                .width(200),
        );

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
                text(format!("{} Load older messages", icons::ARROW_UP))
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
            prev_author.map(|id| id == &msg.author_id).unwrap_or(false)
            && msg.reply_to_id.is_none();

        let timestamp: String = msg.timestamp.format("%H:%M").to_string();

        // ── Reply context ──
        if let Some(reply_id) = msg.reply_to_id {
            if let Some(replied_msg) = messages.iter().find(|m| m.id == reply_id) {
                let reply_preview: String = if replied_msg.content.len() > 60 {
                    format!("{}...", &replied_msg.content[..57])
                } else {
                    replied_msg.content.clone()
                };
                let reply_row = row![
                    Space::with_width(48),
                    text(icons::THREAD).size(12).color(colors::TEXT_TIMESTAMP),
                    Space::with_width(4),
                    text(&replied_msg.author_name).size(11).color(colors::ROLE_MODERATOR),
                    Space::with_width(6),
                    text(reply_preview).size(11).color(colors::TEXT_MUTED),
                ]
                .spacing(0)
                .align_y(Vertical::Center);

                let reply_container = container(reply_row)
                    .padding(Padding::from([2, 0]))
                    .style(|_| container::Style {
                        border: Border {
                            color: colors::ACCENT,
                            width: 0.0,
                            radius: 0.0.into(),
                        },
                        ..container::Style::default()
                    });

                msg_column = msg_column.push(reply_container);
            }
        }

        if is_continuation {
            let mut content_col: Column<'_, ChatAreaMsg> = Column::new().spacing(2);
            content_col = content_col.push(
                render_rich_content(&msg.content, 14),
            );

            // Reactions for continuation messages
            if !msg.reactions.is_empty() {
                content_col = content_col.push(render_reactions(msg.id, &msg.reactions));
            }

            let msg_row = row![
                Space::with_width(48),
                content_col,
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

            let mut header = row![
                text(&msg.author_name).size(14).color(colors::ROLE_FOUNDER),
                Space::with_width(8),
                text(timestamp).size(11).color(colors::TEXT_TIMESTAMP),
            ]
            .align_y(Vertical::Center);

            // Pin indicator
            if msg.pinned {
                header = header
                    .push(Space::with_width(6))
                    .push(text(icons::PIN).size(11).color(colors::STATUS_IDLE));
            }

            // Edited indicator
            if msg.edited_at.is_some() {
                header = header
                    .push(Space::with_width(6))
                    .push(text("(edited)").size(10).color(colors::TEXT_TIMESTAMP));
            }

            let body = render_rich_content(&msg.content, 14);

            // Action buttons row (reply, pin, react)
            let msg_id = msg.id;
            let is_pinned = msg.pinned;

            let reply_btn = button(text(icons::REPLY).size(12).color(colors::TEXT_MUTED))
                .on_press(ChatAreaMsg::ReplyTo(msg_id))
                .padding(Padding::from([2, 6]))
                .style(|_theme, status| {
                    let bg = match status {
                        button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                        _ => iced::Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: colors::TEXT_MUTED,
                        border: Border { radius: 3.0.into(), ..Border::default() },
                        ..button::Style::default()
                    }
                });

            let pin_label = if is_pinned {
                format!("{} Unpin", icons::UNPIN)
            } else {
                format!("{} Pin", icons::PIN)
            };
            let pin_btn = button(text(pin_label).size(10).color(colors::TEXT_MUTED))
                .on_press(ChatAreaMsg::TogglePin(msg_id, is_pinned))
                .padding(Padding::from([2, 6]))
                .style(|_theme, status| {
                    let bg = match status {
                        button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                        _ => iced::Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: colors::TEXT_MUTED,
                        border: Border { radius: 3.0.into(), ..Border::default() },
                        ..button::Style::default()
                    }
                });

            // Quick reactions
            let mut action_row: Row<'_, ChatAreaMsg> = Row::new().spacing(2).align_y(Vertical::Center);
            action_row = action_row.push(reply_btn);
            action_row = action_row.push(pin_btn);
            for emoji in QUICK_REACTIONS {
                let e = emoji.to_string();
                action_row = action_row.push(
                    button(text(*emoji).size(12))
                        .on_press(ChatAreaMsg::ToggleReaction(msg_id, e))
                        .padding(Padding::from([2, 4]))
                        .style(|_theme, status| {
                            let bg = match status {
                                button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                                _ => iced::Color::TRANSPARENT,
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                text_color: colors::TEXT_PRIMARY,
                                border: Border { radius: 3.0.into(), ..Border::default() },
                                ..button::Style::default()
                            }
                        }),
                );
            }

            let actions = container(action_row).padding(Padding::from([2, 0]));

            let mut msg_content = column![header, body].spacing(2);

            // Reactions display
            if !msg.reactions.is_empty() {
                msg_content = msg_content.push(render_reactions(msg.id, &msg.reactions));
            }

            msg_content = msg_content.push(actions);

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

    // ── Reply bar (shown when replying) ──
    let mut chat_column: Column<'_, ChatAreaMsg> = Column::new();
    chat_column = chat_column.push(top_bar);
    chat_column = chat_column.push(messages_area);

    if let Some(reply_msg) = reply_to {
        let reply_preview: String = if reply_msg.content.len() > 80 {
            format!("{}...", &reply_msg.content[..77])
        } else {
            reply_msg.content.clone()
        };

        let reply_bar = container(
            row![
                text("Replying to ").size(12).color(colors::TEXT_MUTED),
                text(&reply_msg.author_name).size(12).color(colors::ROLE_MODERATOR),
                text(": ").size(12).color(colors::TEXT_MUTED),
                text(reply_preview).size(12).color(colors::TEXT_MUTED),
                Space::with_width(Length::Fill),
                button(text(icons::CLOSE).size(12).color(colors::TEXT_MUTED))
                    .on_press(ChatAreaMsg::CancelReply)
                    .padding(Padding::from([2, 6]))
                    .style(|_theme, _status| button::Style {
                        background: Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                        text_color: colors::TEXT_MUTED,
                        ..button::Style::default()
                    }),
            ]
            .spacing(0)
            .align_y(Vertical::Center),
        )
        .padding(Padding::from([6, 16]))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_DARK)),
            border: Border {
                color: colors::ACCENT,
                width: 2.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });
        chat_column = chat_column.push(reply_bar);
    }

    // ── Input bar ──
    let placeholder = format!("Message #{}", channel_name);

    let input = text_input(&placeholder, input_value)
        .on_input(ChatAreaMsg::InputChanged)
        .on_submit(ChatAreaMsg::SendMessage)
        .padding(Padding::from([10, 12]))
        .size(14);

    let send_btn = button(text(format!("{} Send", icons::SEND)).size(13).color(colors::TEXT_PRIMARY))
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

    chat_column = chat_column.push(input_bar);

    container(chat_column)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_MEDIUM)),
            ..container::Style::default()
        })
        .into()
}

/// Render message content with @mention highlighting.
fn render_rich_content(content: &str, size: u16) -> Element<'_, ChatAreaMsg> {
    let mut parts: Row<'_, ChatAreaMsg> = Row::new().spacing(0);
    let mut current = String::new();

    for word in content.split(' ') {
        if word.starts_with('@') && word.len() > 1 {
            // Flush normal text
            if !current.is_empty() {
                parts = parts.push(text(current.clone()).size(size).color(colors::TEXT_PRIMARY));
                current.clear();
            }
            // Render mention with highlight
            parts = parts.push(
                container(
                    text(word).size(size).color(colors::ACCENT),
                )
                .padding(Padding::from([0, 2]))
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color {
                        r: colors::ACCENT.r,
                        g: colors::ACCENT.g,
                        b: colors::ACCENT.b,
                        a: 0.15,
                    })),
                    border: Border {
                        radius: 3.0.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                }),
            );
            current.push(' ');
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        parts = parts.push(text(current).size(size).color(colors::TEXT_PRIMARY));
    }

    parts.into()
}

/// Render reaction badges under a message.
fn render_reactions(message_id: Uuid, reactions: &[ReactionCount]) -> Element<'_, ChatAreaMsg> {
    let mut reaction_row: Row<'_, ChatAreaMsg> = Row::new().spacing(4);

    for rc in reactions {
        let emoji = rc.emoji.clone();
        let label = format!("{} {}", rc.emoji, rc.count);
        let is_me = rc.me;
        let mid = message_id;

        reaction_row = reaction_row.push(
            button(text(label).size(11).color(if is_me { colors::TEXT_PRIMARY } else { colors::TEXT_MUTED }))
                .on_press(ChatAreaMsg::ToggleReaction(mid, emoji))
                .padding(Padding::from([2, 6]))
                .style(move |_theme, status| {
                    let bg = if is_me {
                        match status {
                            button::Status::Hovered | button::Status::Pressed => colors::BG_ACTIVE,
                            _ => colors::BG_HOVER,
                        }
                    } else {
                        match status {
                            button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                            _ => colors::BG_DARK,
                        }
                    };
                    let border_color = if is_me { colors::ACCENT } else { iced::Color::TRANSPARENT };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: if is_me { colors::TEXT_PRIMARY } else { colors::TEXT_MUTED },
                        border: Border {
                            radius: 10.0.into(),
                            width: if is_me { 1.0 } else { 0.0 },
                            color: border_color,
                        },
                        ..button::Style::default()
                    }
                }),
        );
    }

    container(reaction_row).padding(Padding::from([2, 0])).into()
}
