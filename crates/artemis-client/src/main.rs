mod mock;
mod net;
mod theme;
mod views;

use iced::widget::{container, row};
use iced::{Element, Font, Length, Task as IcedTask};
use futures::SinkExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use artemis_core::models::message::Message;
use artemis_core::models::user::{ServerMember, User, UserStatus};
use artemis_core::protocol::{
    ClientEvent, FriendPayload, FriendRequestPayload, ServerEvent, ServerPayload,
};

use artemis_p2p::crypto::Identity;

use views::{
    channel_sidebar, chat_area, friend_list, login_screen, member_list, server_list,
    ChannelSidebarMsg, ChatAreaMsg, FriendListMsg, LoginMsg, LoginState, MemberListMsg,
    ServerListMsg,
};

/// JetBrains Mono Nerd Font — Regular weight.
const JETBRAINS_MONO_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf");

/// JetBrains Mono Nerd Font — Bold weight.
const JETBRAINS_MONO_BOLD: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf");

/// Font descriptor for JetBrains Mono Nerd Font.
pub const FONT_REGULAR: Font = Font::with_name("JetBrainsMono Nerd Font");
pub const FONT_BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::with_name("JetBrainsMono Nerd Font")
};

/// GitHub OAuth App Client ID.
const GITHUB_CLIENT_ID: &str = "Ov23liYMgdGLfkOKDQya";

/// Default server URL (can be overridden later).
const DEFAULT_SERVER_URL: &str = "http://localhost:3000";

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application("Artemis", Artemis::update, Artemis::view)
        .theme(|_| theme::artemis_theme())
        .default_font(FONT_REGULAR)
        .font(JETBRAINS_MONO_REGULAR)
        .font(JETBRAINS_MONO_BOLD)
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

    // Server connection
    client_tx: Option<mpsc::UnboundedSender<ClientEvent>>,

    // Identity
    github_username: String,
    github_token: String,
    avatar_url: Option<String>,
    identity: Option<Identity>,

    // Friend list (from server)
    friends: Vec<FriendPayload>,
    pending_requests: Vec<FriendRequestPayload>,
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

    // Reply state
    reply_to_id: Option<Uuid>,

    // Search state
    search_query: String,

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

    // Server connection
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
                identity: None,
                friends: Vec::new(),
                pending_requests: Vec::new(),
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
                reply_to_id: None,
                search_query: String::new(),
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

                let _ = open::that(&verification_uri);

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
                self.github_token = token.clone();
                self.avatar_url = avatar_url;
                self.username = username;
                self.login_state = LoginState::Idle;

                // Generate E2E identity
                let identity = Identity::generate();
                let public_key = identity.public_key_b64();
                self.identity = Some(identity);
                tracing::info!("E2E identity generated, public key: {}...", &public_key[..8]);

                // Connect to server relay
                let server_url = DEFAULT_SERVER_URL.to_string();
                let tk = token;
                let pk = public_key;

                return IcedTask::run(
                    iced::stream::channel(64, move |mut output| async move {
                        match net::connect(&server_url, tk).await {
                            Ok((tx, mut rx)) => {
                                // Publish public key
                                let _ = tx.send(ClientEvent::PublishPublicKey {
                                    public_key: pk,
                                });
                                // Fetch friends and requests
                                let _ = tx.send(ClientEvent::FetchFriends);
                                let _ = tx.send(ClientEvent::FetchFriendRequests);

                                // Send connection handle
                                let _ = output.send(AppMessage::Connected(tx)).await;

                                // Forward all server events to the UI
                                while let Some(event) = rx.recv().await {
                                    if output
                                        .send(AppMessage::ServerEventReceived(event))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
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

            AppMessage::Connected(tx) => {
                tracing::info!("Connected to relay server");
                self.client_tx = Some(tx);
                self.screen = AppScreen::Chat;
            }

            AppMessage::ConnectionFailed(e) => {
                tracing::warn!("Server connection failed: {} — using offline mode", e);
                self.screen = AppScreen::Chat;
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

                // Mock friends
                self.friends = vec![
                    FriendPayload {
                        user_id: Uuid::new_v4(),
                        username: "alice-dev".to_string(),
                        display_name: Some("Alice".to_string()),
                        avatar_url: None,
                        public_key: None,
                        status: UserStatus::Online,
                    },
                    FriendPayload {
                        user_id: Uuid::new_v4(),
                        username: "bob-coder".to_string(),
                        display_name: Some("Bob".to_string()),
                        avatar_url: None,
                        public_key: None,
                        status: UserStatus::Online,
                    },
                    FriendPayload {
                        user_id: Uuid::new_v4(),
                        username: "charlie-rust".to_string(),
                        display_name: Some("Charlie".to_string()),
                        avatar_url: None,
                        public_key: None,
                        status: UserStatus::Offline,
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

                // Send friend request through server
                if let Some(tx) = &self.client_tx {
                    let _ = tx.send(ClientEvent::SendFriendRequest {
                        target_username: username.clone(),
                    });
                    tracing::info!("Friend request sent to @{}", username);
                }

                self.add_friend_input.clear();
                self.show_add_friend = false;
            }
            AppMessage::FriendList(FriendListMsg::AcceptRequest(from_user_id)) => {
                if let Some(tx) = &self.client_tx {
                    let _ = tx.send(ClientEvent::AcceptFriendRequest { from_user_id });
                }
                self.pending_requests
                    .retain(|r| r.from_user_id != from_user_id);
            }
            AppMessage::FriendList(FriendListMsg::DeclineRequest(from_user_id)) => {
                if let Some(tx) = &self.client_tx {
                    let _ = tx.send(ClientEvent::DeclineFriendRequest { from_user_id });
                }
                self.pending_requests
                    .retain(|r| r.from_user_id != from_user_id);
            }

            // ── Server events ──
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
                                public_key: None,
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
                    if self.is_home {
                        // DM mode: send E2E encrypted message through server relay
                        self.send_dm();
                    } else if let Some(channel_id) = self.active_channel_id {
                        let reply_id = self.reply_to_id.take();
                        // Server channel mode
                        if let Some(tx) = &self.client_tx {
                            let _ = tx.send(ClientEvent::SendMessage {
                                channel_id,
                                content: self.message_input.clone(),
                                reply_to_id: reply_id,
                            });
                            self.message_input.clear();
                        } else {
                            // Offline/mock
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
                                reply_to_id: reply_id,
                                pinned: false,
                                reactions: vec![],
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
            AppMessage::ChatArea(ChatAreaMsg::ReplyTo(msg_id)) => {
                self.reply_to_id = Some(msg_id);
            }
            AppMessage::ChatArea(ChatAreaMsg::CancelReply) => {
                self.reply_to_id = None;
            }
            AppMessage::ChatArea(ChatAreaMsg::ToggleReaction(msg_id, emoji)) => {
                if let Some(tx) = &self.client_tx {
                    // Check if we already reacted with this emoji
                    let already_reacted = self.messages.iter()
                        .find(|m| m.id == msg_id)
                        .map(|m| m.reactions.iter().any(|r| r.emoji == emoji && r.me))
                        .unwrap_or(false);

                    if already_reacted {
                        let _ = tx.send(ClientEvent::RemoveReaction { message_id: msg_id, emoji });
                    } else {
                        let _ = tx.send(ClientEvent::AddReaction { message_id: msg_id, emoji });
                    }
                } else {
                    // Offline/mock: toggle reaction locally
                    if let Some(msg) = self.messages.iter_mut().find(|m| m.id == msg_id) {
                        if let Some(rc) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
                            if rc.me {
                                rc.count = rc.count.saturating_sub(1);
                                rc.me = false;
                                if rc.count == 0 {
                                    msg.reactions.retain(|r| r.emoji != emoji);
                                }
                            } else {
                                rc.count += 1;
                                rc.me = true;
                            }
                        } else {
                            msg.reactions.push(artemis_core::models::message::ReactionCount {
                                emoji,
                                count: 1,
                                me: true,
                            });
                        }
                    }
                }
            }
            AppMessage::ChatArea(ChatAreaMsg::TogglePin(msg_id, is_pinned)) => {
                if let Some(tx) = &self.client_tx {
                    if is_pinned {
                        let _ = tx.send(ClientEvent::UnpinMessage { message_id: msg_id });
                    } else {
                        let _ = tx.send(ClientEvent::PinMessage { message_id: msg_id });
                    }
                } else {
                    // Offline/mock
                    if let Some(msg) = self.messages.iter_mut().find(|m| m.id == msg_id) {
                        msg.pinned = !msg.pinned;
                    }
                }
            }
            AppMessage::ChatArea(ChatAreaMsg::SearchInputChanged(val)) => {
                self.search_query = val;
            }
            AppMessage::ChatArea(ChatAreaMsg::SubmitSearch) => {
                if !self.search_query.trim().is_empty() {
                    if let Some(tx) = &self.client_tx {
                        let server_id = self.servers.get(self.active_server_idx).map(|s| s.id).unwrap_or(Uuid::nil());
                        let _ = tx.send(ClientEvent::SearchMessages {
                            server_id,
                            channel_id: self.active_channel_id,
                            query: self.search_query.clone(),
                            limit: 25,
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

    /// Send an E2E encrypted DM to the active friend via server relay.
    fn send_dm(&mut self) {
        let Some(active_username) = &self.active_friend else {
            return;
        };
        let Some(friend) = self.friends.iter().find(|f| &f.username == active_username) else {
            return;
        };
        let Some(identity) = &self.identity else {
            tracing::error!("No identity available for E2E encryption");
            return;
        };
        let Some(tx) = &self.client_tx else {
            return;
        };

        let content = self.message_input.clone();
        let recipient_id = friend.user_id;

        // Encrypt message if friend has a public key
        let encrypted_content = if let Some(ref pk) = friend.public_key {
            match identity.encrypt_for_b64(pk, content.as_bytes()) {
                Ok(enc) => enc,
                Err(e) => {
                    tracing::error!("E2E encryption failed: {}", e);
                    return;
                }
            }
        } else {
            tracing::warn!("Friend has no public key, sending unencrypted");
            content.clone()
        };

        let _ = tx.send(ClientEvent::SendDirectMessage {
            recipient_id,
            encrypted_content,
        });

        // Optimistic local display
        let dm_channel_id = self.dm_channel_id(recipient_id);
        let msg = Message {
            id: Uuid::new_v4(),
            channel_id: dm_channel_id,
            author_id: self.user_id.unwrap_or(Uuid::nil()),
            author_name: self.username.clone(),
            author_avatar: self.avatar_url.clone(),
            content,
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            edited_at: None,
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        };
        self.messages.push(msg);
        self.message_input.clear();
    }

    /// Derive a deterministic DM channel ID from two user IDs.
    fn dm_channel_id(&self, other_user_id: Uuid) -> Uuid {
        let my_id = self.user_id.unwrap_or(Uuid::nil());
        let (a, b) = if my_id < other_user_id {
            (my_id, other_user_id)
        } else {
            (other_user_id, my_id)
        };
        Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("dm:{}:{}", a, b).as_bytes())
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
                                public_key: None,
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
                            public_key: None,
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

            // ── Friend / DM events ──

            ServerEvent::PublicKeyAcknowledged => {
                tracing::info!("Public key registered on server");
            }

            ServerEvent::FriendRequestReceived {
                from_user_id,
                from_username,
                from_avatar_url,
            } => {
                tracing::info!("Friend request from @{}", from_username);
                if !self
                    .pending_requests
                    .iter()
                    .any(|r| r.from_user_id == from_user_id)
                {
                    self.pending_requests.push(FriendRequestPayload {
                        from_user_id,
                        from_username,
                        from_avatar_url,
                        created_at: chrono::Utc::now(),
                    });
                }
            }

            ServerEvent::FriendRequestAccepted {
                user_id,
                username,
                avatar_url,
                public_key,
            } => {
                tracing::info!("Now friends with @{}", username);
                if !self.friends.iter().any(|f| f.user_id == user_id) {
                    self.friends.push(FriendPayload {
                        user_id,
                        username,
                        display_name: None,
                        avatar_url,
                        public_key,
                        status: UserStatus::Online,
                    });
                }
            }

            ServerEvent::FriendRequestDeclined { by_user_id } => {
                tracing::info!("Friend request declined by {}", by_user_id);
            }

            ServerEvent::FriendList { friends } => {
                self.friends = friends;
                tracing::info!("Friend list loaded: {} friends", self.friends.len());
            }

            ServerEvent::PendingFriendRequests { requests } => {
                self.pending_requests = requests;
                tracing::info!(
                    "Pending friend requests: {}",
                    self.pending_requests.len()
                );
            }

            ServerEvent::DirectMessageReceived {
                from_user_id,
                from_username,
                encrypted_content,
                timestamp,
                message_id,
            } => {
                if self.messages.iter().any(|m| m.id == message_id) {
                    return;
                }

                let dm_channel_id = self.dm_channel_id(from_user_id);

                // Try to decrypt
                let content = if from_user_id == self.user_id.unwrap_or(Uuid::nil()) {
                    // Our own echoed message — skip (already added optimistically)
                    return;
                } else if let Some(identity) = &self.identity {
                    let sender_pk = self
                        .friends
                        .iter()
                        .find(|f| f.user_id == from_user_id)
                        .and_then(|f| f.public_key.as_ref());

                    if let Some(pk) = sender_pk {
                        match identity.decrypt_from_b64(pk, &encrypted_content) {
                            Ok(plaintext) => {
                                String::from_utf8(plaintext).unwrap_or(encrypted_content)
                            }
                            Err(_) => encrypted_content,
                        }
                    } else {
                        encrypted_content
                    }
                } else {
                    encrypted_content
                };

                let msg = Message {
                    id: message_id,
                    channel_id: dm_channel_id,
                    author_id: from_user_id,
                    author_name: from_username,
                    author_avatar: None,
                    content,
                    attachments: vec![],
                    timestamp,
                    edited_at: None,
                    reply_to_id: None,
                    pinned: false,
                    reactions: vec![],
                };
                self.messages.push(msg);
            }

            ServerEvent::FriendPresenceUpdate { user_id, status } => {
                if let Some(friend) = self.friends.iter_mut().find(|f| f.user_id == user_id) {
                    friend.status = status;
                }
            }

            // ── Reactions ──

            ServerEvent::ReactionAdded { message_id, user_id: reactor_id, emoji } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    let is_me = self.user_id.map(|uid| uid == reactor_id).unwrap_or(false);
                    if let Some(rc) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
                        rc.count += 1;
                        if is_me { rc.me = true; }
                    } else {
                        msg.reactions.push(artemis_core::models::message::ReactionCount {
                            emoji,
                            count: 1,
                            me: is_me,
                        });
                    }
                }
            }

            ServerEvent::ReactionRemoved { message_id, user_id: reactor_id, emoji } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    let is_me = self.user_id.map(|uid| uid == reactor_id).unwrap_or(false);
                    if let Some(rc) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
                        rc.count = rc.count.saturating_sub(1);
                        if is_me { rc.me = false; }
                    }
                    msg.reactions.retain(|r| r.count > 0);
                }
            }

            // ── Pins ──

            ServerEvent::MessagePinned { message_id, .. } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    msg.pinned = true;
                }
            }

            ServerEvent::MessageUnpinned { message_id, .. } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    msg.pinned = false;
                }
            }

            ServerEvent::PinnedMessages { messages, .. } => {
                // Mark pinned messages in local state
                for pinned_msg in &messages {
                    if let Some(msg) = self.messages.iter_mut().find(|m| m.id == pinned_msg.id) {
                        msg.pinned = true;
                    }
                }
            }

            // ── Unread ──

            ServerEvent::UnreadState { channels } => {
                tracing::info!("Unread state received for {} channels", channels.len());
                // Could be used for badge counts in the channel sidebar in the future
            }

            // ── Server / channel management ──

            ServerEvent::ServerUpdated { server_id, name, icon_url } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    if let Some(n) = name { server.name = n; }
                    if let Some(icon) = icon_url { server.icon_url = Some(icon); }
                }
            }

            ServerEvent::ServerDeleted { server_id } => {
                self.servers.retain(|s| s.id != server_id);
                if self.servers.is_empty() {
                    self.is_home = true;
                }
            }

            ServerEvent::ChannelCreated { server_id, category_id, channel } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    if let Some(cat) = server.categories.iter_mut().find(|c| c.id == category_id) {
                        cat.channels.push(channel);
                    }
                }
            }

            ServerEvent::ChannelDeleted { server_id, channel_id } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    for cat in &mut server.categories {
                        cat.channels.retain(|ch| ch.id != channel_id);
                    }
                }
                if self.active_channel_id == Some(channel_id) {
                    self.active_channel_id = None;
                }
            }

            ServerEvent::ChannelUpdated { channel_id, name, topic } => {
                for server in &mut self.servers {
                    for cat in &mut server.categories {
                        if let Some(ch) = cat.channels.iter_mut().find(|c| c.id == channel_id) {
                            if let Some(n) = name.clone() { ch.name = n; }
                            if let Some(t) = topic.clone() { ch.topic = Some(t); }
                        }
                    }
                }
            }

            ServerEvent::CategoryCreated { server_id, category } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    server.categories.push(category);
                }
            }

            ServerEvent::InviteCode { server_id, invite_code } => {
                tracing::info!("Invite code for server {}: {}", server_id, invite_code);
            }

            ServerEvent::ProfileUpdated { user_id: uid, display_name, custom_status } => {
                if let Some(member) = self.members.iter_mut().find(|m| m.user.id == uid) {
                    if let Some(dn) = display_name { member.user.display_name = Some(dn); }
                    if let Some(cs) = custom_status { member.user.custom_status = Some(cs); }
                }
            }

            ServerEvent::DirectMessageHistory { friend_id, messages, .. } => {
                let dm_channel_id = self.dm_channel_id(friend_id);
                for dm in messages {
                    let msg_id = dm.id;
                    if !self.messages.iter().any(|m| m.id == msg_id) {
                        // Attempt decrypt
                        let content = if let Some(identity) = &self.identity {
                            let sender_pk = self.friends.iter()
                                .find(|f| f.user_id == dm.sender_id)
                                .and_then(|f| f.public_key.as_ref());
                            if let Some(pk) = sender_pk {
                                match identity.decrypt_from_b64(pk, &dm.encrypted_content) {
                                    Ok(pt) => String::from_utf8(pt).unwrap_or(dm.encrypted_content.clone()),
                                    Err(_) => dm.encrypted_content.clone(),
                                }
                            } else {
                                dm.encrypted_content.clone()
                            }
                        } else {
                            dm.encrypted_content.clone()
                        };

                        self.messages.push(Message {
                            id: dm.id,
                            channel_id: dm_channel_id,
                            author_id: dm.sender_id,
                            author_name: dm.sender_username,
                            author_avatar: None,
                            content,
                            attachments: vec![],
                            timestamp: dm.created_at,
                            edited_at: None,
                            reply_to_id: None,
                            pinned: false,
                            reactions: vec![],
                        });
                    }
                }
                self.messages.sort_by_key(|m| m.timestamp);
            }

            // ── Search results ──

            ServerEvent::SearchResults { messages, .. } => {
                // Replace current view with search results (add them to messages list)
                for msg in messages {
                    if !self.messages.iter().any(|m| m.id == msg.id) {
                        self.messages.push(msg);
                    }
                }
                self.messages.sort_by_key(|m| m.timestamp);
            }

            // ── Custom emojis ──

            ServerEvent::CustomEmojiAdded { .. }
            | ServerEvent::CustomEmojiRemoved { .. }
            | ServerEvent::CustomEmojiList { .. } => {
                tracing::debug!("Custom emoji event received");
            }
        }
    }

    fn view(&self) -> Element<'_, AppMessage> {
        match &self.screen {
            AppScreen::Login => login_screen::view(&self.login_state).map(AppMessage::Login),

            AppScreen::Chat => {
                let active_server = self.servers.get(self.active_server_idx);

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
                    let friend_list_view = friend_list::view(
                        &self.friends,
                        &self.pending_requests,
                        self.active_friend.as_deref(),
                        &self.add_friend_input,
                        self.show_add_friend,
                    )
                    .map(AppMessage::FriendList);

                    // Show DM messages for active friend
                    let dm_channel = self.active_friend.as_ref().and_then(|uname| {
                        let friend = self.friends.iter().find(|f| &f.username == uname)?;
                        Some(self.dm_channel_id(friend.user_id))
                    });

                    let reply_msg = self.reply_to_id.and_then(|rid| self.messages.iter().find(|m| m.id == rid));
                    let chat_view = chat_area::view_with_payload(
                        None,
                        &self.messages,
                        &self.message_input,
                        dm_channel,
                        reply_msg,
                        &self.search_query,
                    )
                    .map(AppMessage::ChatArea);

                    main_row = main_row.push(friend_list_view);
                    main_row = main_row.push(chat_view);
                } else {
                    let channel_sidebar_view =
                        channel_sidebar::view_from_payload(active_server, self.active_channel_id)
                            .map(AppMessage::ChannelSidebar);

                    let active_channel_payload = active_server.and_then(|s| {
                        s.categories
                            .iter()
                            .flat_map(|c| &c.channels)
                            .find(|ch| Some(ch.id) == self.active_channel_id)
                    });

                    let reply_msg = self.reply_to_id.and_then(|rid| self.messages.iter().find(|m| m.id == rid));
                    let chat_view = chat_area::view_with_payload(
                        active_channel_payload,
                        &self.messages,
                        &self.message_input,
                        self.active_channel_id,
                        reply_msg,
                        &self.search_query,
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
