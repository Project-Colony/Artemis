use iced::alignment::{Horizontal, Vertical};
use iced::widget::{column, container, row, scrollable, text, Column, Space};
use iced::{Border, Element, Length, Padding};

use artemis_core::models::user::{MemberRole, UserStatus};
use crate::theme::{colors, icons};

#[derive(Debug, Clone)]
pub enum MemberListMsg {
    #[allow(dead_code)]
    ToggleMemberList,
}

pub fn view(members: &[artemis_core::ServerMember]) -> Element<'_, MemberListMsg> {
    let mut content = Column::new().spacing(4).padding(Padding::from([8, 12]));

    // Group members by role
    let founders: Vec<_> = members
        .iter()
        .filter(|m| m.role == MemberRole::Founder)
        .collect();
    let moderators: Vec<_> = members
        .iter()
        .filter(|m| m.role == MemberRole::Moderator)
        .collect();
    let online_members: Vec<_> = members
        .iter()
        .filter(|m| {
            m.role == MemberRole::Member
                && m.user.status != UserStatus::Offline
        })
        .collect();

    if !founders.is_empty() {
        content = content.push(role_header("Founder", founders.len()));
        for member in &founders {
            content = content.push(member_entry(member, colors::ROLE_FOUNDER));
        }
        content = content.push(Space::with_height(8));
    }

    if !moderators.is_empty() {
        content = content.push(role_header("Moderator", moderators.len()));
        for member in &moderators {
            content = content.push(member_entry(member, colors::ROLE_MODERATOR));
        }
        content = content.push(Space::with_height(8));
    }

    if !online_members.is_empty() {
        content = content.push(role_header("Online", online_members.len()));
        for member in &online_members {
            content =
                content.push(member_entry(member, colors::TEXT_PRIMARY));
        }
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

fn role_header<'a>(name: &str, count: usize) -> Element<'a, MemberListMsg> {
    container(
        text(format!("{} — {}", name, count))
            .size(11)
            .color(colors::TEXT_MUTED),
    )
    .padding(Padding::from([6, 4]))
    .into()
}

fn member_entry<'a>(
    member: &artemis_core::ServerMember,
    name_color: iced::Color,
) -> Element<'a, MemberListMsg> {
    let status_color = match member.user.status {
        UserStatus::Online => colors::STATUS_ONLINE,
        UserStatus::Idle => colors::STATUS_IDLE,
        UserStatus::DoNotDisturb => iced::color!(0xEF, 0x44, 0x44),
        UserStatus::Offline => colors::TEXT_TIMESTAMP,
    };

    // Avatar placeholder
    let avatar = container(
        text(
            member
                .user
                .username
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string(),
        )
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

    // Status icon (Nerd Font)
    let status_icon = match member.user.status {
        UserStatus::Online => icons::STATUS_ONLINE,
        UserStatus::Idle => icons::STATUS_IDLE,
        UserStatus::DoNotDisturb => icons::STATUS_DND,
        UserStatus::Offline => icons::STATUS_OFFLINE,
    };
    let status_dot = text(status_icon).size(10).color(status_color);

    // Name + status text
    let status_text = member
        .user
        .custom_status
        .clone()
        .unwrap_or_else(|| format!("{:?}", member.user.status));

    let name_col = column![
        text(member.user.username.clone())
            .size(13)
            .color(name_color),
        text(status_text)
            .size(11)
            .color(colors::TEXT_TIMESTAMP),
    ]
    .spacing(1);

    let entry = row![
        avatar,
        Space::with_width(8),
        name_col,
        Space::with_width(Length::Fill),
        status_dot,
    ]
    .align_y(Vertical::Center)
    .padding(Padding::from([4, 4]));

    container(entry)
        .width(Length::Fill)
        .style(|_| container::Style {
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}
