use iced::alignment::Vertical;
use iced::widget::{button, container, row, scrollable, text, Column, Space};
use iced::{Border, Element, Length, Padding};
use uuid::Uuid;

use artemis_core::protocol::ServerPayload;
use crate::theme::{colors, icons};

#[derive(Debug, Clone)]
pub enum ChannelSidebarMsg {
    SelectChannel(Uuid),
    ToggleCategory(Uuid),
}

pub fn view_from_payload<'a>(
    server: Option<&'a ServerPayload>,
    active_channel_id: Option<Uuid>,
) -> Element<'a, ChannelSidebarMsg> {
    let Some(server) = server else {
        return container(text("No server selected").color(colors::TEXT_MUTED))
            .width(240)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(colors::BG_DARK)),
                ..container::Style::default()
            })
            .into();
    };

    let mut content = Column::new().spacing(2);

    let header = container(
        row![
            text(&server.name).size(16).color(colors::TEXT_PRIMARY),
            Space::with_width(Length::Fill),
        ]
        .padding(Padding::from([0, 4]))
        .align_y(Vertical::Center),
    )
    .padding(Padding::from([12, 16]))
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(colors::BG_DARK)),
        ..container::Style::default()
    });

    content = content.push(header);
    content = content.push(Space::with_height(4));

    for category in &server.categories {
        let cat_header = button(
            row![
                text(icons::CHEVRON_DOWN).size(10).color(colors::TEXT_MUTED),
                Space::with_width(4),
                text(&category.name).size(11).color(colors::TEXT_MUTED),
            ]
            .align_y(Vertical::Center),
        )
        .on_press(ChannelSidebarMsg::ToggleCategory(category.id))
        .padding(Padding::from([6, 16]))
        .width(Length::Fill)
        .style(|_theme, _status| button::Style {
            background: None,
            text_color: colors::TEXT_MUTED,
            ..button::Style::default()
        });

        content = content.push(cat_header);

        for channel in &category.channels {
            let is_active = active_channel_id == Some(channel.id);
            let channel_id = channel.id;

            let icon_str = match channel.channel_type {
                artemis_core::ChannelType::Text => icons::CHANNEL_TEXT,
                artemis_core::ChannelType::Voice => icons::CHANNEL_VOICE,
            };

            let name_color = if is_active {
                colors::TEXT_PRIMARY
            } else {
                colors::TEXT_MUTED
            };

            let chan_btn = button(
                row![
                    text(icon_str).size(14).color(colors::TEXT_MUTED),
                    Space::with_width(6),
                    text(&channel.name).size(14).color(name_color),
                ]
                .align_y(Vertical::Center),
            )
            .on_press(ChannelSidebarMsg::SelectChannel(channel_id))
            .padding(Padding::from([4, 8]))
            .width(Length::Fill)
            .style(move |_theme, status| {
                let bg = if is_active {
                    Some(iced::Background::Color(colors::BG_ACTIVE))
                } else {
                    match status {
                        button::Status::Hovered | button::Status::Pressed => {
                            Some(iced::Background::Color(colors::BG_HOVER))
                        }
                        _ => None,
                    }
                };
                button::Style {
                    background: bg,
                    text_color: name_color,
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..button::Style::default()
                }
            });

            content = content.push(container(chan_btn).padding(Padding::from([0, 8])));
        }

        content = content.push(Space::with_height(4));
    }

    let scrollable_content = scrollable(content).height(Length::Fill);

    container(scrollable_content)
        .width(240)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(colors::BG_DARK)),
            ..container::Style::default()
        })
        .into()
}
