use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, row, scrollable, text, text_input, Column, Space};
use iced::{Border, Element, Length, Padding};

use crate::theme::{colors, icons};
use artemis_core::models::user::UserStatus;
use artemis_core::protocol::{FriendPayload, FriendRequestPayload};

#[derive(Debug, Clone)]
pub enum FriendListMsg {
    SelectFriend(String),
    AddFriendInputChanged(String),
    AddFriend,
    ToggleAddFriend,
    AcceptRequest(uuid::Uuid),
    DeclineRequest(uuid::Uuid),
}

pub fn view<'a>(
    friends: &'a [FriendPayload],
    pending_requests: &'a [FriendRequestPayload],
    active_friend: Option<&'a str>,
    add_friend_input: &'a str,
    show_add_form: bool,
) -> Element<'a, FriendListMsg> {
    let mut content = Column::new().spacing(2);

    // Header
    let header = container(
        row![
            text(format!("{} Friends", icons::MEMBERS))
                .size(16)
                .color(colors::TEXT_PRIMARY),
            Space::with_width(Length::Fill),
            button(text(icons::USER_PLUS).size(14).color(colors::STATUS_ONLINE))
                .on_press(FriendListMsg::ToggleAddFriend)
                .padding(Padding::from([2, 8]))
                .style(|_theme, status| {
                    let bg = match status {
                        button::Status::Hovered | button::Status::Pressed => colors::BG_HOVER,
                        _ => colors::BG_DARK,
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: colors::STATUS_ONLINE,
                        border: Border {
                            radius: 4.0.into(),
                            ..Border::default()
                        },
                        ..button::Style::default()
                    }
                }),
        ]
        .align_y(Vertical::Center),
    )
    .padding(Padding::from([12, 16]))
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(colors::BG_DARK)),
        ..container::Style::default()
    });

    content = content.push(header);

    // Add friend form
    if show_add_form {
        let input = text_input("GitHub username...", add_friend_input)
            .on_input(FriendListMsg::AddFriendInputChanged)
            .on_submit(FriendListMsg::AddFriend)
            .padding(Padding::from([8, 10]))
            .size(13);

        let add_btn = button(text("Add").size(12).color(colors::TEXT_PRIMARY))
            .on_press(FriendListMsg::AddFriend)
            .padding(Padding::from([8, 12]))
            .style(|_theme, _status| button::Style {
                background: Some(iced::Background::Color(colors::ACCENT)),
                text_color: colors::TEXT_PRIMARY,
                border: Border {
                    radius: 4.0.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            });

        let form = container(row![input, Space::with_width(4), add_btn].align_y(Vertical::Center))
            .padding(Padding::from([8, 12]))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(colors::BG_MEDIUM)),
                ..container::Style::default()
            });

        content = content.push(form);
    }

    content = content.push(Space::with_height(4));

    // Pending friend requests
    if !pending_requests.is_empty() {
        content = content.push(
            container(
                text(format!(
                    "{} PENDING \u{2014} {}",
                    icons::PENDING,
                    pending_requests.len()
                ))
                .size(11)
                .color(colors::STATUS_IDLE),
            )
            .padding(Padding::from([6, 16])),
        );

        for req in pending_requests {
            content = content.push(friend_request_entry(req));
        }

        content = content.push(Space::with_height(8));
    }

    // Online friends
    let online: Vec<_> = friends
        .iter()
        .filter(|f| f.status != UserStatus::Offline)
        .collect();
    let offline: Vec<_> = friends
        .iter()
        .filter(|f| f.status == UserStatus::Offline)
        .collect();

    if !online.is_empty() {
        content = content.push(
            container(
                text(format!(
                    "{} ONLINE \u{2014} {}",
                    icons::STATUS_ONLINE,
                    online.len()
                ))
                .size(11)
                .color(colors::TEXT_MUTED),
            )
            .padding(Padding::from([6, 16])),
        );

        for friend in &online {
            let is_active = active_friend == Some(friend.username.as_str());
            content = content.push(friend_entry(friend, is_active));
        }

        content = content.push(Space::with_height(8));
    }

    if !offline.is_empty() {
        content = content.push(
            container(
                text(format!(
                    "{} OFFLINE \u{2014} {}",
                    icons::STATUS_OFFLINE,
                    offline.len()
                ))
                .size(11)
                .color(colors::TEXT_MUTED),
            )
            .padding(Padding::from([6, 16])),
        );

        for friend in &offline {
            let is_active = active_friend == Some(friend.username.as_str());
            content = content.push(friend_entry(friend, is_active));
        }
    }

    if friends.is_empty() && pending_requests.is_empty() {
        content = content.push(Space::with_height(20));
        content = content.push(
            container(
                column![
                    text("No friends yet")
                        .size(13)
                        .color(colors::TEXT_MUTED)
                        .align_x(Horizontal::Center),
                    Space::with_height(4),
                    text("Click + to add by GitHub username")
                        .size(11)
                        .color(colors::TEXT_TIMESTAMP)
                        .align_x(Horizontal::Center),
                ]
                .align_x(Horizontal::Center),
            )
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .padding(Padding::from([0, 16])),
        );
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

fn friend_entry<'a>(friend: &FriendPayload, is_active: bool) -> Element<'a, FriendListMsg> {
    let username = friend.username.clone();
    let display = friend
        .display_name
        .clone()
        .unwrap_or_else(|| friend.username.clone());

    let status_color = match friend.status {
        UserStatus::Online => colors::STATUS_ONLINE,
        UserStatus::Idle => colors::STATUS_IDLE,
        UserStatus::DoNotDisturb => iced::color!(0xEF, 0x44, 0x44),
        UserStatus::Offline => colors::TEXT_TIMESTAMP,
    };

    let avatar_initial = display
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    let avatar = container(
        text(avatar_initial)
            .size(12)
            .color(colors::TEXT_PRIMARY)
            .align_x(Horizontal::Center),
    )
    .width(32)
    .height(32)
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .style(move |_| container::Style {
        background: Some(iced::Background::Color(colors::BG_HOVER)),
        border: Border {
            radius: 16.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    });

    let status_icon = match friend.status {
        UserStatus::Online => icons::STATUS_ONLINE,
        UserStatus::Idle => icons::STATUS_IDLE,
        UserStatus::DoNotDisturb => icons::STATUS_DND,
        UserStatus::Offline => icons::STATUS_OFFLINE,
    };
    let status_dot = text(status_icon).size(10).color(status_color);

    let name_col = column![
        text(display).size(13).color(if is_active {
            colors::TEXT_PRIMARY
        } else {
            colors::TEXT_MUTED
        }),
        text(format!("@{}", friend.username))
            .size(10)
            .color(colors::TEXT_TIMESTAMP),
    ]
    .spacing(1);

    let entry_row = row![
        avatar,
        Space::with_width(8),
        name_col,
        Space::with_width(Length::Fill),
        status_dot,
    ]
    .align_y(Vertical::Center);

    let btn = button(entry_row)
        .on_press(FriendListMsg::SelectFriend(username))
        .padding(Padding::from([6, 12]))
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
                text_color: colors::TEXT_PRIMARY,
                border: Border {
                    radius: 4.0.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        });

    container(btn).padding(Padding::from([0, 4])).into()
}

fn friend_request_entry<'a>(req: &FriendRequestPayload) -> Element<'a, FriendListMsg> {
    let from_id = req.from_user_id;
    let display = req.from_username.clone();

    let avatar_initial = display
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    let avatar = container(
        text(avatar_initial)
            .size(12)
            .color(colors::TEXT_PRIMARY)
            .align_x(Horizontal::Center),
    )
    .width(32)
    .height(32)
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .style(move |_| container::Style {
        background: Some(iced::Background::Color(colors::STATUS_IDLE)),
        border: Border {
            radius: 16.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    });

    let name_col = column![
        text(display).size(13).color(colors::TEXT_PRIMARY),
        text("wants to be friends")
            .size(10)
            .color(colors::TEXT_TIMESTAMP),
    ]
    .spacing(1);

    let accept_btn = button(text(icons::CHECK).size(14).color(colors::STATUS_ONLINE))
        .on_press(FriendListMsg::AcceptRequest(from_id))
        .padding(Padding::from([4, 8]))
        .style(|_theme, _status| button::Style {
            background: Some(iced::Background::Color(colors::BG_HOVER)),
            text_color: colors::STATUS_ONLINE,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        });

    let decline_btn = button(
        text(icons::DECLINE)
            .size(14)
            .color(iced::color!(0xEF, 0x44, 0x44)),
    )
    .on_press(FriendListMsg::DeclineRequest(from_id))
    .padding(Padding::from([4, 8]))
    .style(|_theme, _status| button::Style {
        background: Some(iced::Background::Color(colors::BG_HOVER)),
        text_color: iced::color!(0xEF, 0x44, 0x44),
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    });

    let entry_row = row![
        avatar,
        Space::with_width(8),
        name_col,
        Space::with_width(Length::Fill),
        accept_btn,
        Space::with_width(4),
        decline_btn,
    ]
    .align_y(Vertical::Center);

    container(entry_row)
        .padding(Padding::from([6, 12]))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::color!(0x35, 0x3A, 0x20))),
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}
