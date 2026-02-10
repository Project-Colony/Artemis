mod mock;
mod net;
mod theme;
mod views;

use std::sync::Arc;

use iced::widget::{container, row};
use iced::{Element, Length, Task as IcedTask};
use futures::SinkExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use artemis_core::models::message::Message;
use artemis_core::models::user::{ServerMember, User, UserStatus};
use artemis_core::protocol::{ClientEvent, ServerEvent, ServerPayload};

use views::{
    channel_sidebar, chat_area, login_screen, member_list, server_list, ChannelSidebarMsg,
    ChatAreaMsg, LoginMsg, MemberListMsg, ServerListMsg,
};

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application("Artemis", Artemis::update, Artemis::view)
        .theme(|_| theme::artemis_theme())
        .window_size((1280.0, 720.0))
        .run_with(Artemis::new)
}

enum AppScreen {
    Login,
    Chat,
}

struct Artemis {
    screen: AppScreen,

    // Login state
    server_url: String,
    auth_token: String,
    login_error: Option<String>,
    connecting: bool,

    // Connection
    client_tx: Option<mpsc::UnboundedSender<ClientEvent>>,

    // Chat state
    user_id: Option<Uuid>,
    username: String,
    servers: Vec<ServerPayload>,
    active_server_idx: usize,
    active_channel_id: Option<Uuid>,
    messages: Vec<Message>,
    members: Vec<ServerMember>,
    message_input: String,
    show_member_list: bool,
}

#[derive(Debug, Clone)]
enum AppMessage {
    Login(LoginMsg),
    ServerList(ServerListMsg),
    ChannelSidebar(ChannelSidebarMsg),
    ChatArea(ChatAreaMsg),
    MemberList(MemberListMsg),

    // Network
    Connected(mpsc::UnboundedSender<ClientEvent>),
    ConnectionFailed(String),
    ServerEventReceived(ServerEvent),
}

impl Artemis {
    fn new() -> (Self, IcedTask<AppMessage>) {
        let saved_token = std::env::var("ARTEMIS_TOKEN").unwrap_or_default();
        let server_url = std::env::var("ARTEMIS_SERVER")
            .unwrap_or_else(|_| "http://localhost:3000".to_string());

        (
            Self {
                screen: AppScreen::Login,
                server_url,
                auth_token: saved_token,
                login_error: None,
                connecting: false,
                client_tx: None,
                user_id: None,
                username: String::new(),
                servers: Vec::new(),
                active_server_idx: 0,
                active_channel_id: None,
                messages: Vec::new(),
                members: Vec::new(),
                message_input: String::new(),
                show_member_list: true,
            },
            IcedTask::none(),
        )
    }

    fn update(&mut self, message: AppMessage) -> IcedTask<AppMessage> {
        match message {
            // ── Login ──
            AppMessage::Login(LoginMsg::ServerUrlChanged(url)) => {
                self.server_url = url;
            }
            AppMessage::Login(LoginMsg::TokenChanged(token)) => {
                self.auth_token = token;
            }
            AppMessage::Login(LoginMsg::Connect) => {
                if self.auth_token.trim().is_empty() {
                    self.login_error = Some("Token is required".to_string());
                    return IcedTask::none();
                }
                self.connecting = true;
                self.login_error = None;

                let server_url = self.server_url.clone();
                let token = self.auth_token.clone();

                return IcedTask::run(
                    iced::stream::channel(64, move |mut output| async move {
                        match net::connect(&server_url, token).await {
                            Ok((tx, mut rx)) => {
                                // Send the client sender back to the app
                                let _ = output.send(AppMessage::Connected(tx)).await;

                                // Forward all server events
                                while let Some(event) = rx.recv().await {
                                    let _ = output
                                        .send(AppMessage::ServerEventReceived(event))
                                        .await;
                                }
                            }
                            Err(e) => {
                                let _ = output
                                    .send(AppMessage::ConnectionFailed(e.to_string()))
                                    .await;
                            }
                        }
                    }),
                    |msg| msg,
                );
            }
            AppMessage::Login(LoginMsg::UseMockData) => {
                let (servers, messages, members) = mock::sample_data();
                self.servers = servers
                    .iter()
                    .map(|s| ServerPayload {
                        id: s.id,
                        name: s.name.clone(),
                        icon_url: s.icon_url.clone(),
                        categories: s
                            .categories
                            .iter()
                            .map(|c| artemis_core::protocol::CategoryPayload {
                                id: c.id,
                                name: c.name.clone(),
                                channels: c
                                    .channels
                                    .iter()
                                    .map(|ch| artemis_core::protocol::ChannelPayload {
                                        id: ch.id,
                                        name: ch.name.clone(),
                                        channel_type: ch.channel_type,
                                        topic: ch.topic.clone(),
                                    })
                                    .collect(),
                            })
                            .collect(),
                        members: members
                            .iter()
                            .map(|m| artemis_core::protocol::MemberPayload {
                                user_id: m.user.id,
                                username: m.user.username.clone(),
                                avatar_url: m.user.avatar_url.clone(),
                                role: m.role,
                                status: m.user.status,
                                custom_status: m.user.custom_status.clone(),
                            })
                            .collect(),
                    })
                    .collect();
                self.messages = messages;
                self.members = members;
                self.active_channel_id = self
                    .servers
                    .first()
                    .and_then(|s| s.categories.first())
                    .and_then(|c| c.channels.first())
                    .map(|ch| ch.id);
                self.user_id = Some(Uuid::nil());
                self.username = "You".to_string();
                self.screen = AppScreen::Chat;
            }

            AppMessage::Connected(tx) => {
                self.connecting = false;
                self.client_tx = Some(tx);
            }
            AppMessage::ConnectionFailed(e) => {
                self.connecting = false;
                self.login_error = Some(format!("Connection failed: {}", e));
            }

            AppMessage::ServerEventReceived(event) => {
                self.handle_server_event(event);
            }

            // ── Chat interactions ──
            AppMessage::ServerList(ServerListMsg::SelectServer(idx)) => {
                self.active_server_idx = idx;
                if let Some(server) = self.servers.get(idx) {
                    self.active_channel_id = server
                        .categories
                        .first()
                        .and_then(|c| c.channels.first())
                        .map(|ch| ch.id);

                    // Update members for new server
                    self.members = server
                        .members
                        .iter()
                        .map(|m| ServerMember {
                            user: User {
                                id: m.user_id,
                                username: m.username.clone(),
                                display_name: Some(m.username.clone()),
                                avatar_url: m.avatar_url.clone(),
                                github_id: None,
                                status: m.status,
                                custom_status: m.custom_status.clone(),
                                created_at: chrono::Utc::now(),
                            },
                            role: m.role,
                            joined_at: chrono::Utc::now(),
                        })
                        .collect();

                    if let (Some(channel_id), Some(tx)) =
                        (self.active_channel_id, &self.client_tx)
                    {
                        let _ = tx.send(ClientEvent::FetchMessages {
                            channel_id,
                            before: None,
                            limit: 50,
                        });
                    }
                }
            }
            AppMessage::ChannelSidebar(ChannelSidebarMsg::SelectChannel(id)) => {
                self.active_channel_id = Some(id);
                if let Some(tx) = &self.client_tx {
                    let _ = tx.send(ClientEvent::FetchMessages {
                        channel_id: id,
                        before: None,
                        limit: 50,
                    });
                }
            }
            AppMessage::ChannelSidebar(ChannelSidebarMsg::ToggleCategory(_cat_id)) => {
                // Category collapse state would need local UI tracking
            }
            AppMessage::ChatArea(ChatAreaMsg::InputChanged(val)) => {
                self.message_input = val;
            }
            AppMessage::ChatArea(ChatAreaMsg::SendMessage) => {
                if !self.message_input.trim().is_empty() {
                    if let Some(channel_id) = self.active_channel_id {
                        if let Some(tx) = &self.client_tx {
                            let _ = tx.send(ClientEvent::SendMessage {
                                channel_id,
                                content: self.message_input.clone(),
                            });
                            self.message_input.clear();
                        } else {
                            // Mock mode
                            let msg = Message {
                                id: Uuid::new_v4(),
                                channel_id,
                                author_id: self.user_id.unwrap_or(Uuid::nil()),
                                author_name: self.username.clone(),
                                author_avatar: None,
                                content: self.message_input.clone(),
                                attachments: vec![],
                                timestamp: chrono::Utc::now(),
                                edited_at: None,
                            };
                            self.messages.push(msg);
                            self.message_input.clear();
                        }
                    }
                }
            }
            AppMessage::ChatArea(ChatAreaMsg::LoadOlderMessages) => {
                if let Some(channel_id) = self.active_channel_id {
                    let oldest = self
                        .messages
                        .iter()
                        .filter(|m| m.channel_id == channel_id)
                        .min_by_key(|m| m.timestamp)
                        .map(|m| m.id);
                    if let Some(tx) = &self.client_tx {
                        let _ = tx.send(ClientEvent::FetchMessages {
                            channel_id,
                            before: oldest,
                            limit: 50,
                        });
                    }
                }
            }
            AppMessage::MemberList(MemberListMsg::ToggleMemberList) => {
                self.show_member_list = !self.show_member_list;
            }
        }
        IcedTask::none()
    }

    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Authenticated {
                user_id,
                username,
                servers,
            } => {
                self.user_id = Some(user_id);
                self.username = username;
                self.servers = servers;
                self.active_channel_id = self
                    .servers
                    .first()
                    .and_then(|s| s.categories.first())
                    .and_then(|c| c.channels.first())
                    .map(|ch| ch.id);

                self.members.clear();
                if let Some(server) = self.servers.first() {
                    for m in &server.members {
                        self.members.push(ServerMember {
                            user: User {
                                id: m.user_id,
                                username: m.username.clone(),
                                display_name: Some(m.username.clone()),
                                avatar_url: m.avatar_url.clone(),
                                github_id: None,
                                status: m.status,
                                custom_status: m.custom_status.clone(),
                                created_at: chrono::Utc::now(),
                            },
                            role: m.role,
                            joined_at: chrono::Utc::now(),
                        });
                    }
                }

                self.screen = AppScreen::Chat;

                if let (Some(channel_id), Some(tx)) = (self.active_channel_id, &self.client_tx) {
                    let _ = tx.send(ClientEvent::FetchMessages {
                        channel_id,
                        before: None,
                        limit: 50,
                    });
                }
            }
            ServerEvent::AuthError { reason } => {
                self.login_error = Some(reason);
                self.screen = AppScreen::Login;
            }
            ServerEvent::MessageReceived { message } => {
                if !self.messages.iter().any(|m| m.id == message.id) {
                    self.messages.push(message);
                }
            }
            ServerEvent::MessageEdited {
                message_id,
                content,
                edited_at,
            } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    msg.content = content;
                    msg.edited_at = Some(edited_at);
                }
            }
            ServerEvent::MessageDeleted { message_id } => {
                self.messages.retain(|m| m.id != message_id);
            }
            ServerEvent::MessageHistory {
                channel_id: _,
                messages,
                has_more: _,
            } => {
                for msg in messages {
                    if !self.messages.iter().any(|m| m.id == msg.id) {
                        self.messages.push(msg);
                    }
                }
                self.messages.sort_by_key(|m| m.timestamp);
            }
            ServerEvent::PresenceUpdate { user_id, status } => {
                if let Some(member) = self.members.iter_mut().find(|m| m.user.id == user_id) {
                    member.user.status = status;
                }
            }
            ServerEvent::MemberJoined {
                user_id: uid,
                username,
                avatar_url,
                role,
                ..
            } => {
                if !self.members.iter().any(|m| m.user.id == uid) {
                    self.members.push(ServerMember {
                        user: User {
                            id: uid,
                            username,
                            display_name: None,
                            avatar_url,
                            github_id: None,
                            status: UserStatus::Online,
                            custom_status: None,
                            created_at: chrono::Utc::now(),
                        },
                        role,
                        joined_at: chrono::Utc::now(),
                    });
                }
            }
            ServerEvent::MemberLeft { user_id: uid, .. } => {
                self.members.retain(|m| m.user.id != uid);
            }
            ServerEvent::UserTyping { .. } => {}
            ServerEvent::Error { message } => {
                tracing::error!("Server error: {}", message);
            }
        }
    }

    fn view(&self) -> Element<'_, AppMessage> {
        match &self.screen {
            AppScreen::Login => login_screen::view(
                &self.server_url,
                &self.auth_token,
                self.login_error.as_deref(),
                self.connecting,
            )
            .map(AppMessage::Login),

            AppScreen::Chat => {
                let active_server = self.servers.get(self.active_server_idx);

                let server_list_view =
                    server_list::view_from_payloads(&self.servers, self.active_server_idx)
                        .map(AppMessage::ServerList);

                let channel_sidebar_view =
                    channel_sidebar::view_from_payload(active_server, self.active_channel_id)
                        .map(AppMessage::ChannelSidebar);

                let active_channel_payload = active_server.and_then(|s| {
                    s.categories
                        .iter()
                        .flat_map(|c| &c.channels)
                        .find(|ch| Some(ch.id) == self.active_channel_id)
                });

                let chat_view = chat_area::view_with_payload(
                    active_channel_payload,
                    &self.messages,
                    &self.message_input,
                    self.active_channel_id,
                )
                .map(AppMessage::ChatArea);

                let mut main_row = row![server_list_view, channel_sidebar_view, chat_view,];

                if self.show_member_list {
                    let member_view =
                        member_list::view(&self.members).map(AppMessage::MemberList);
                    main_row = main_row.push(member_view);
                }

                container(main_row)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        }
    }
}
