mod mock;
mod net;
mod theme;
mod views;

use iced::widget::{container, row};
use iced::{Element, Length, Task as IcedTask};
use futures::SinkExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use artemis_core::models::message::Message;
use artemis_core::models::user::{ServerMember, User, UserStatus};
use artemis_core::protocol::{ClientEvent, ServerEvent, ServerPayload};

use artemis_p2p::protocol::PeerProfile;

use views::{
    channel_sidebar, chat_area, friend_list, login_screen, member_list, server_list,
    ChannelSidebarMsg, ChatAreaMsg, FriendListMsg, LoginMsg, LoginState, MemberListMsg,
    ServerListMsg,
};

/// GitHub OAuth App Client ID.
/// For development: create one at https://github.com/settings/applications/new
/// Enable "Device Flow" in the app settings.
const GITHUB_CLIENT_ID: &str = "Ov23liYMgdGLfkOKDQya";

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
    login_state: LoginState,

    // Connection (legacy WebSocket — kept for server mode)
    client_tx: Option<mpsc::UnboundedSender<ClientEvent>>,

    // P2P identity
    github_username: String,
    github_token: String,
    avatar_url: Option<String>,

    // Friend list
    friends: Vec<PeerProfile>,
    active_friend: Option<String>,
    add_friend_input: String,
    show_add_friend: bool,

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

    // Navigation: home (friends) vs server
    is_home: bool,

    // Mode
    is_mock: bool,
}

#[derive(Debug, Clone)]
enum AppMessage {
    Login(LoginMsg),
    FriendList(FriendListMsg),
    ServerList(ServerListMsg),
    ChannelSidebar(ChannelSidebarMsg),
    ChatArea(ChatAreaMsg),
    MemberList(MemberListMsg),

    // GitHub Device Flow
    DeviceCodeReceived {
        device_code: String,
        user_code: String,
        verification_uri: String,
        interval: u64,
    },
    DeviceFlowError(String),
    GitHubTokenReceived(String),
    GitHubUserFetched {
        username: String,
        avatar_url: Option<String>,
        token: String,
    },
    P2PInitialized,

    // Friend operations
    FriendLookupResult {
        username: String,
        found: bool,
        public_key: Option<String>,
        signaling_gist_id: Option<String>,
    },

    // Network (legacy)
    Connected(mpsc::UnboundedSender<ClientEvent>),
    ConnectionFailed(String),
    ServerEventReceived(ServerEvent),
}

impl Artemis {
    fn new() -> (Self, IcedTask<AppMessage>) {
        (
            Self {
                screen: AppScreen::Login,
                login_state: LoginState::Idle,
                client_tx: None,
                github_username: String::new(),
                github_token: String::new(),
                avatar_url: None,
                friends: Vec::new(),
                active_friend: None,
                add_friend_input: String::new(),
                show_add_friend: false,
                user_id: None,
                username: String::new(),
                servers: Vec::new(),
                active_server_idx: 0,
                active_channel_id: None,
                messages: Vec::new(),
                members: Vec::new(),
                message_input: String::new(),
                show_member_list: true,
                is_home: true,
                is_mock: false,
            },
            IcedTask::none(),
        )
    }

    fn update(&mut self, message: AppMessage) -> IcedTask<AppMessage> {
        match message {
            // ── Login ──
            AppMessage::Login(LoginMsg::SignInWithGitHub) => {
                self.login_state = LoginState::Polling;

                let client_id = GITHUB_CLIENT_ID.to_string();

                return IcedTask::perform(
                    async move { artemis_auth::request_device_code(&client_id).await },
                    |result| match result {
                        Ok(resp) => AppMessage::DeviceCodeReceived {
                            device_code: resp.device_code,
                            user_code: resp.user_code,
                            verification_uri: resp.verification_uri,
                            interval: resp.interval,
                        },
                        Err(e) => AppMessage::DeviceFlowError(e.to_string()),
                    },
                );
            }

            AppMessage::DeviceCodeReceived {
                device_code,
                user_code,
                verification_uri,
                interval,
            } => {
                self.login_state = LoginState::WaitingForCode {
                    user_code: user_code.clone(),
                    verification_uri: verification_uri.clone(),
                };

                // Open browser for user
                let _ = open::that(&verification_uri);

                // Start polling for token
                let client_id = GITHUB_CLIENT_ID.to_string();
                let dc = device_code.clone();

                return IcedTask::run(
                    iced::stream::channel(8, move |mut output| async move {
                        let poll_interval = std::time::Duration::from_secs(interval.max(5));
                        loop {
                            tokio::time::sleep(poll_interval).await;
                            match artemis_auth::poll_device_token(&client_id, &dc).await {
                                Ok(resp) => {
                                    if let Some(token) = resp.access_token {
                                        let _ = output
                                            .send(AppMessage::GitHubTokenReceived(token))
                                            .await;
                                        return;
                                    }
                                    if let Some(error) = &resp.error {
                                        match error.as_str() {
                                            "authorization_pending" => continue,
                                            "slow_down" => {
                                                tokio::time::sleep(
                                                    std::time::Duration::from_secs(5),
                                                )
                                                .await;
                                                continue;
                                            }
                                            "expired_token" => {
                                                let _ = output
                                                    .send(AppMessage::DeviceFlowError(
                                                        "Code expired, please try again"
                                                            .to_string(),
                                                    ))
                                                    .await;
                                                return;
                                            }
                                            "access_denied" => {
                                                let _ = output
                                                    .send(AppMessage::DeviceFlowError(
                                                        "Access denied by user".to_string(),
                                                    ))
                                                    .await;
                                                return;
                                            }
                                            other => {
                                                let desc = resp
                                                    .error_description
                                                    .unwrap_or_else(|| other.to_string());
                                                let _ = output
                                                    .send(AppMessage::DeviceFlowError(desc))
                                                    .await;
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = output
                                        .send(AppMessage::DeviceFlowError(e.to_string()))
                                        .await;
                                    return;
                                }
                            }
                        }
                    }),
                    |msg| msg,
                );
            }

            AppMessage::GitHubTokenReceived(token) => {
                self.login_state = LoginState::Initializing;
                let t = token.clone();

                return IcedTask::perform(
                    async move { artemis_auth::GitHubOAuth::fetch_user(&t).await },
                    move |result| match result {
                        Ok(user) => AppMessage::GitHubUserFetched {
                            username: user.login,
                            avatar_url: user.avatar_url,
                            token: token.clone(),
                        },
                        Err(e) => AppMessage::DeviceFlowError(format!(
                            "Failed to fetch GitHub profile: {}",
                            e
                        )),
                    },
                );
            }

            AppMessage::GitHubUserFetched {
                username,
                avatar_url,
                token,
            } => {
                self.github_username = username.clone();
                self.github_token = token;
                self.avatar_url = avatar_url;
                self.username = username;
                self.user_id = Some(Uuid::new_v4());
                self.login_state = LoginState::Idle;
                self.screen = AppScreen::Chat;

                tracing::info!("Authenticated as @{}", self.github_username);
            }

            AppMessage::P2PInitialized => {
                tracing::info!("P2P stack initialized");
            }

            AppMessage::DeviceFlowError(err) => {
                self.login_state = LoginState::Error(err);
            }

            AppMessage::Login(LoginMsg::UseMockData) => {
                let (servers, messages, members) = mock::sample_data();
                self.is_mock = true;
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
                self.github_username = "demo-user".to_string();

                // Add some mock friends
                self.friends = vec![
                    PeerProfile {
                        github_username: "alice-dev".to_string(),
                        display_name: Some("Alice".to_string()),
                        avatar_url: None,
                        public_key: String::new(),
                        signaling_gist_id: None,
                        status: UserStatus::Online,
                        added_at: chrono::Utc::now(),
                    },
                    PeerProfile {
                        github_username: "bob-coder".to_string(),
                        display_name: Some("Bob".to_string()),
                        avatar_url: None,
                        public_key: String::new(),
                        signaling_gist_id: None,
                        status: UserStatus::Online,
                        added_at: chrono::Utc::now(),
                    },
                    PeerProfile {
                        github_username: "charlie-rust".to_string(),
                        display_name: Some("Charlie".to_string()),
                        avatar_url: None,
                        public_key: String::new(),
                        signaling_gist_id: None,
                        status: UserStatus::Offline,
                        added_at: chrono::Utc::now(),
                    },
                ];

                self.screen = AppScreen::Chat;
            }

            // ── Friend list ──
            AppMessage::FriendList(FriendListMsg::SelectFriend(username)) => {
                self.active_friend = Some(username);
            }
            AppMessage::FriendList(FriendListMsg::AddFriendInputChanged(val)) => {
                self.add_friend_input = val;
            }
            AppMessage::FriendList(FriendListMsg::ToggleAddFriend) => {
                self.show_add_friend = !self.show_add_friend;
            }
            AppMessage::FriendList(FriendListMsg::AddFriend) => {
                let username = self.add_friend_input.trim().to_string();
                if username.is_empty() {
                    return IcedTask::none();
                }

                if !self.friends.iter().any(|f| f.github_username == username) {
                    self.friends.push(PeerProfile {
                        github_username: username.clone(),
                        display_name: None,
                        avatar_url: None,
                        public_key: String::new(),
                        signaling_gist_id: None,
                        status: UserStatus::Offline,
                        added_at: chrono::Utc::now(),
                    });
                }
                self.add_friend_input.clear();
                self.show_add_friend = false;

                tracing::info!("Added friend: @{}", username);
            }

            AppMessage::FriendLookupResult {
                username,
                found,
                public_key,
                signaling_gist_id,
            } => {
                if found {
                    if let Some(friend) = self
                        .friends
                        .iter_mut()
                        .find(|f| f.github_username == username)
                    {
                        if let Some(pk) = public_key {
                            friend.public_key = pk;
                        }
                        friend.signaling_gist_id = signaling_gist_id;
                    }
                    tracing::info!("Found peer profile for @{}", username);
                } else {
                    tracing::warn!("No Artemis profile found for @{}", username);
                }
            }

            // ── Legacy server interactions (kept for mock/server mode) ──
            AppMessage::Connected(tx) => {
                self.client_tx = Some(tx);
            }
            AppMessage::ConnectionFailed(e) => {
                self.login_state = LoginState::Error(format!("Connection failed: {}", e));
            }
            AppMessage::ServerEventReceived(event) => {
                self.handle_server_event(event);
            }

            AppMessage::ServerList(ServerListMsg::GoHome) => {
                self.is_home = true;
            }

            AppMessage::ServerList(ServerListMsg::SelectServer(idx)) => {
                self.is_home = false;
                self.active_server_idx = idx;
                if let Some(server) = self.servers.get(idx) {
                    self.active_channel_id = server
                        .categories
                        .first()
                        .and_then(|c| c.channels.first())
                        .map(|ch| ch.id);

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
            AppMessage::ChannelSidebar(ChannelSidebarMsg::ToggleCategory(_cat_id)) => {}

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
            ServerEvent::AuthError { reason } => {
                self.login_state = LoginState::Error(reason);
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
            AppScreen::Login => login_screen::view(&self.login_state).map(AppMessage::Login),

            AppScreen::Chat => {
                let active_server = self.servers.get(self.active_server_idx);

                // Server strip (always visible, far left — like Discord)
                let active_srv_idx = if self.is_home {
                    None
                } else {
                    Some(self.active_server_idx)
                };
                let server_strip = server_list::view_from_payloads(
                    &self.servers,
                    active_srv_idx,
                    self.is_home,
                )
                .map(AppMessage::ServerList);

                let mut main_row = row![server_strip];

                if self.is_home {
                    // Home mode: show friend list + DM chat
                    let friend_list_view = friend_list::view(
                        &self.friends,
                        self.active_friend.as_deref(),
                        &self.add_friend_input,
                        self.show_add_friend,
                    )
                    .map(AppMessage::FriendList);

                    let chat_view = chat_area::view_with_payload(
                        None,
                        &self.messages,
                        &self.message_input,
                        self.active_channel_id,
                    )
                    .map(AppMessage::ChatArea);

                    main_row = main_row.push(friend_list_view);
                    main_row = main_row.push(chat_view);
                } else {
                    // Server mode: show channels + chat + members
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

                    main_row = main_row.push(channel_sidebar_view);
                    main_row = main_row.push(chat_view);

                    if self.show_member_list {
                        let member_view =
                            member_list::view(&self.members).map(AppMessage::MemberList);
                        main_row = main_row.push(member_view);
                    }
                }

                container(main_row)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        }
    }
}
