mod mock;
mod net;
mod theme;
mod views;

use std::collections::HashMap;

use iced::alignment::Vertical;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Color, Element, Font, Length, Padding, Task as IcedTask};
use tokio::sync::mpsc;
use uuid::Uuid;

use artemis_core::limits::MAX_MESSAGE_CHARS;
use artemis_core::models::message::Message;
use artemis_core::models::user::{MemberRole, ServerMember, User, UserStatus};
use artemis_core::protocol::{
    ClientEvent, FriendPayload, FriendRequestPayload, MemberPayload, ServerEvent, ServerPayload,
};

use artemis_p2p::crypto::{Identity, Purpose};

use theme::{colors, icons};
use views::chat_area::Outgoing;
use views::{
    channel_sidebar, chat_area, friend_list, login_screen, member_list, server_list,
    ChannelSidebarMsg, ChatAreaMsg, FriendListMsg, LoginMsg, LoginState, MemberListMsg,
    ServerListMsg,
};

/// JetBrains Mono Nerd Font — Regular weight.
const JETBRAINS_MONO_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf");

/// JetBrains Mono Nerd Font — Bold weight.
const JETBRAINS_MONO_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Bold.ttf");

/// Font descriptor for JetBrains Mono Nerd Font.
pub const FONT_REGULAR: Font = Font::with_name("JetBrainsMono Nerd Font");
pub const FONT_BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::with_name("JetBrainsMono Nerd Font")
};

/// The relay, until the client lets the user choose one.
const DEFAULT_SERVER_URL: &str = "http://localhost:3000";

fn relay_url() -> reqwest::Url {
    DEFAULT_SERVER_URL
        .parse()
        .expect("the default relay URL is valid")
}

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application(Artemis::new, Artemis::update, Artemis::view)
        .title("Artemis")
        .theme(theme::artemis_theme())
        .default_font(FONT_REGULAR)
        .font(JETBRAINS_MONO_REGULAR)
        .font(JETBRAINS_MONO_BOLD)
        .window_size((1280.0, 720.0))
        .run()
}

enum AppScreen {
    Login,
    Chat,
}

struct Artemis {
    screen: AppScreen,

    // Login state
    login_state: LoginState,

    // Relay session
    /// Events to the relay, while a socket is open.
    client_tx: Option<mpsc::UnboundedSender<ClientEvent>>,
    /// The session task. Dropping it closes the socket and stops reconnecting.
    session: Option<iced::task::Handle>,
    /// Messages sent but not yet echoed by the relay, by client nonce. Each
    /// is also in `messages`, under its nonce, until the echo replaces it.
    outbox: HashMap<Uuid, Outgoing>,
    /// The last error, shown above the chat until dismissed.
    banner: Option<String>,

    // Identity
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

    // Sign-in through the relay
    DeviceCodeReceived {
        user_code: String,
    },
    DeviceFlowError(String),
    SignedIn {
        /// The relay's session token.
        token: String,
        username: String,
        avatar_url: Option<String>,
    },

    // Relay session
    Connected(mpsc::UnboundedSender<ClientEvent>),
    /// The socket ended, with why. The session reconnects on its own.
    Disconnected(String),
    /// A socket could not be opened. The session tries again on its own.
    ConnectionFailed(String),
    ServerEventReceived(ServerEvent),
    DismissBanner,
}

impl Artemis {
    fn new() -> (Self, IcedTask<AppMessage>) {
        (
            Self {
                screen: AppScreen::Login,
                login_state: LoginState::Idle,
                client_tx: None,
                session: None,
                outbox: HashMap::new(),
                banner: None,
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
                return IcedTask::run(net::sign_in(relay_url()), |msg| msg);
            }

            AppMessage::DeviceCodeReceived { user_code } => {
                self.login_state = LoginState::WaitingForCode { user_code };
                // The page is on screen too, so a browser that does not open
                // is not an error.
                let _ = open::that(net::GITHUB_DEVICE_PAGE);
            }

            AppMessage::DeviceFlowError(err) => {
                self.login_state = LoginState::Error(err);
            }

            AppMessage::SignedIn {
                token,
                username,
                avatar_url,
            } => {
                self.avatar_url = avatar_url;
                self.username = username;
                self.login_state = LoginState::Initializing;

                // Generate E2E identity
                let identity = Identity::generate();
                tracing::info!(
                    "E2E identity generated, public key: {}...",
                    &identity.public_key_b64()[..8]
                );
                self.identity = Some(identity);

                // Replacing an older session stops it.
                let (task, handle) =
                    IcedTask::run(net::session(relay_url(), token), |msg| msg).abortable();
                self.session = Some(handle.abort_on_drop());
                return task;
            }

            AppMessage::Connected(tx) => {
                tracing::info!("Connected to relay server");
                self.client_tx = Some(tx);
                self.screen = AppScreen::Chat;
                // A connection error is over once connected again.
                self.banner = None;
            }

            AppMessage::Disconnected(reason) => {
                tracing::warn!("Lost the connection to the relay: {}", reason);
                self.client_tx = None;
                self.fail_pending();
            }

            AppMessage::ConnectionFailed(e) => {
                tracing::warn!("Cannot reach the relay: {}", e);
                self.banner = Some(format!("Cannot reach the relay: {e}"));
                self.screen = AppScreen::Chat;
            }

            AppMessage::DismissBanner => {
                self.banner = None;
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
                self.open_server(idx, None);
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
                // A NUL can never be sent and would disable Send unseen, so it
                // is dropped. Text over the cap stays for the user to shorten,
                // but a huge paste is cut at twice the cap so it cannot stall
                // every redraw.
                self.message_input = val
                    .chars()
                    .filter(|&c| c != '\0')
                    .take(2 * MAX_MESSAGE_CHARS)
                    .collect();
            }
            AppMessage::ChatArea(ChatAreaMsg::SendMessage) => {
                // Over the cap the relay would refuse it, so the text stays
                // in the input for the user to shorten.
                if chat_area::can_send(&self.message_input) {
                    if self.is_home {
                        // DM mode: send E2E encrypted message through server relay
                        self.send_dm();
                    } else if let Some(channel_id) = self.active_channel_id {
                        let content = self.message_input.clone();
                        let reply_to_id = self.reply_to_id;
                        let sent = if self.is_mock {
                            let msg = self.local_message(
                                Uuid::new_v4(),
                                channel_id,
                                content,
                                reply_to_id,
                            );
                            self.messages.push(msg);
                            true
                        } else {
                            let nonce = Uuid::new_v4();
                            let event = ClientEvent::SendMessage {
                                channel_id,
                                content: content.clone(),
                                reply_to_id,
                                client_nonce: Some(nonce),
                            };
                            self.send_pending(nonce, event, channel_id, content, reply_to_id)
                        };
                        // Offline, the text and the reply wait in the input.
                        if sent {
                            self.message_input.clear();
                            self.reply_to_id = None;
                        }
                    }
                }
            }
            AppMessage::ChatArea(ChatAreaMsg::Retry(nonce)) => {
                // The same nonce, so the echo replaces this message.
                if let (Some(tx), Some(out)) = (&self.client_tx, self.outbox.get_mut(&nonce)) {
                    if tx.send(out.event.clone()).is_ok() {
                        out.failed = false;
                    }
                }
            }
            AppMessage::ChatArea(ChatAreaMsg::Discard(nonce)) => {
                self.outbox.remove(&nonce);
                self.messages.retain(|m| m.id != nonce);
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
                    let already_reacted = self
                        .messages
                        .iter()
                        .find(|m| m.id == msg_id)
                        .map(|m| m.reactions.iter().any(|r| r.emoji == emoji && r.me))
                        .unwrap_or(false);

                    if already_reacted {
                        let _ = tx.send(ClientEvent::RemoveReaction {
                            message_id: msg_id,
                            emoji,
                        });
                    } else {
                        let _ = tx.send(ClientEvent::AddReaction {
                            message_id: msg_id,
                            emoji,
                        });
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
                            msg.reactions
                                .push(artemis_core::models::message::ReactionCount {
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
                        let server_id = self
                            .servers
                            .get(self.active_server_idx)
                            .map(|s| s.id)
                            .unwrap_or(Uuid::nil());
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

        let content = self.message_input.clone();
        let recipient_id = friend.user_id;

        // Encrypt message if friend has a public key
        let encrypted_content = if let Some(ref pk) = friend.public_key {
            match identity.encrypt_for_b64(Purpose::DirectMessage, pk, content.as_bytes()) {
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

        let nonce = Uuid::new_v4();
        let event = ClientEvent::SendDirectMessage {
            recipient_id,
            encrypted_content,
            client_nonce: Some(nonce),
        };
        let dm_channel_id = self.dm_channel_id(recipient_id);
        if self.send_pending(nonce, event, dm_channel_id, content, None) {
            self.message_input.clear();
        }
    }

    /// Whether events cannot reach the relay right now. The demo never
    /// connects, so it is never offline.
    fn offline(&self) -> bool {
        !self.is_mock && self.client_tx.is_none()
    }

    /// Queues `event`, which sends a message under `nonce`, and shows the
    /// message as pending until the relay echoes it. Returns false, with
    /// nothing queued or shown, while offline.
    fn send_pending(
        &mut self,
        nonce: Uuid,
        event: ClientEvent,
        channel_id: Uuid,
        content: String,
        reply_to_id: Option<Uuid>,
    ) -> bool {
        let Some(tx) = &self.client_tx else {
            return false;
        };
        if tx.send(event.clone()).is_err() {
            return false;
        }
        self.outbox.insert(
            nonce,
            Outgoing {
                event,
                failed: false,
            },
        );
        let pending = self.local_message(nonce, channel_id, content, reply_to_id);
        self.messages.push(pending);
        true
    }

    /// A message written by the user, as shown before the relay stores it.
    fn local_message(
        &self,
        id: Uuid,
        channel_id: Uuid,
        content: String,
        reply_to_id: Option<Uuid>,
    ) -> Message {
        Message {
            id,
            channel_id,
            author_id: self.user_id.unwrap_or(Uuid::nil()),
            author_name: self.username.clone(),
            author_avatar: self.avatar_url.clone(),
            content,
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            edited_at: None,
            reply_to_id,
            pinned: false,
            reactions: vec![],
        }
    }

    /// Adds `message` from the relay once. When `nonce` names a pending
    /// message, the stored one takes its place.
    fn settle(&mut self, nonce: Option<Uuid>, message: Message) {
        let pending = nonce
            .filter(|n| self.outbox.remove(n).is_some())
            .and_then(|n| self.messages.iter().position(|m| m.id == n));
        if self.messages.iter().any(|m| m.id == message.id) {
            if let Some(i) = pending {
                self.messages.remove(i);
            }
        } else if let Some(i) = pending {
            self.messages[i] = message;
        } else {
            self.messages.push(message);
        }
    }

    /// Marks every message still waiting for its echo as not sent. None is
    /// re-sent on its own, so none can arrive twice: each waits for Retry.
    fn fail_pending(&mut self) {
        for out in self.outbox.values_mut() {
            out.failed = true;
        }
    }

    /// Shows server `idx`: its members, and `channel` when it is one of its
    /// channels or else its first channel, whose history is fetched.
    fn open_server(&mut self, idx: usize, channel: Option<Uuid>) {
        self.active_server_idx = idx;
        let Some(server) = self.servers.get(idx) else {
            self.active_channel_id = None;
            self.members.clear();
            return;
        };
        let channels: Vec<Uuid> = server
            .categories
            .iter()
            .flat_map(|c| &c.channels)
            .map(|ch| ch.id)
            .collect();
        self.active_channel_id = channel
            .filter(|id| channels.contains(id))
            .or(channels.first().copied());
        self.members = server.members.iter().map(server_member).collect();

        if let (Some(channel_id), Some(tx)) = (self.active_channel_id, &self.client_tx) {
            let _ = tx.send(ClientEvent::FetchMessages {
                channel_id,
                before: None,
                limit: 50,
            });
        }
    }

    /// Text to show for a direct message exchanged with `friend_id`.
    ///
    /// Without a public key for the friend, the message came through the
    /// plaintext fallback and is shown as is. With one, a message that does
    /// not decrypt is never shown as its raw content: the relay could have
    /// written it.
    fn dm_text(&self, friend_id: Uuid, sent_by_us: bool, content: String) -> String {
        let Some(pk) = self
            .friends
            .iter()
            .find(|f| f.user_id == friend_id)
            .and_then(|f| f.public_key.as_deref())
        else {
            return content;
        };
        self.identity
            .as_ref()
            .and_then(|identity| {
                if sent_by_us {
                    identity.decrypt_sent_b64(Purpose::DirectMessage, pk, &content)
                } else {
                    identity.decrypt_from_b64(Purpose::DirectMessage, pk, &content)
                }
                .ok()
            })
            .and_then(|plaintext| String::from_utf8(plaintext).ok())
            .unwrap_or_else(|| "[This message could not be decrypted]".to_string())
    }

    /// Whether the user may pin in the active server. The relay lets only
    /// founders and moderators pin; the demo pins locally, so it always may.
    fn can_pin(&self) -> bool {
        self.is_mock
            || self.members.iter().any(|m| {
                Some(m.user.id) == self.user_id
                    && matches!(m.role, MemberRole::Founder | MemberRole::Moderator)
            })
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
                // After a reconnection, the server and channel on screen stay
                // when they still exist.
                let shown = self.servers.get(self.active_server_idx).map(|s| s.id);
                self.servers = servers;
                let idx = shown
                    .and_then(|id| self.servers.iter().position(|s| s.id == id))
                    .unwrap_or(0);
                self.open_server(idx, self.active_channel_id);
                self.screen = AppScreen::Chat;

                // Each socket publishes the key and reloads the friend list.
                if let (Some(tx), Some(identity)) = (&self.client_tx, &self.identity) {
                    let _ = tx.send(ClientEvent::PublishPublicKey {
                        public_key: identity.public_key_b64(),
                    });
                    let _ = tx.send(ClientEvent::FetchFriends);
                    let _ = tx.send(ClientEvent::FetchFriendRequests);
                }
            }
            ServerEvent::AuthError { reason } => {
                // The session is void: stop its socket and its reconnections.
                self.session = None;
                self.client_tx = None;
                self.fail_pending();
                self.login_state = LoginState::Error(reason);
                self.screen = AppScreen::Login;
            }
            ServerEvent::MessageReceived {
                message,
                client_nonce,
            } => self.settle(client_nonce, message),
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
                tracing::warn!("Relay error: {}", message);
                // ponytail: an Error does not name the event it answers, so it
                // fails every pending message. An echo still replaces one the
                // relay did store, but a Retry before it would send it twice.
                // Give Error the client_nonce of a refused send if that shows.
                self.fail_pending();
                self.banner = Some(message);
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
                tracing::info!("Pending friend requests: {}", self.pending_requests.len());
            }

            ServerEvent::DirectMessageReceived {
                from_user_id,
                from_username,
                encrypted_content,
                timestamp,
                message_id,
                client_nonce,
            } => {
                if Some(from_user_id) == self.user_id {
                    // Our own message, echoed to the socket that sent it. The
                    // pending copy knows the conversation and the text.
                    let sent = client_nonce
                        .and_then(|n| self.messages.iter().find(|m| m.id == n))
                        .cloned();
                    if let Some(sent) = sent {
                        let stored = Message {
                            id: message_id,
                            timestamp,
                            ..sent
                        };
                        self.settle(client_nonce, stored);
                    }
                    return;
                }
                let content = self.dm_text(from_user_id, false, encrypted_content);

                let msg = Message {
                    id: message_id,
                    channel_id: self.dm_channel_id(from_user_id),
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
                self.settle(None, msg);
            }

            ServerEvent::FriendPresenceUpdate { user_id, status } => {
                if let Some(friend) = self.friends.iter_mut().find(|f| f.user_id == user_id) {
                    friend.status = status;
                }
            }

            // ── Reactions ──
            ServerEvent::ReactionAdded {
                message_id,
                user_id: reactor_id,
                emoji,
            } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    let is_me = self.user_id.map(|uid| uid == reactor_id).unwrap_or(false);
                    if let Some(rc) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
                        rc.count += 1;
                        if is_me {
                            rc.me = true;
                        }
                    } else {
                        msg.reactions
                            .push(artemis_core::models::message::ReactionCount {
                                emoji,
                                count: 1,
                                me: is_me,
                            });
                    }
                }
            }

            ServerEvent::ReactionRemoved {
                message_id,
                user_id: reactor_id,
                emoji,
            } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                    let is_me = self.user_id.map(|uid| uid == reactor_id).unwrap_or(false);
                    if let Some(rc) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
                        rc.count = rc.count.saturating_sub(1);
                        if is_me {
                            rc.me = false;
                        }
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
            ServerEvent::ServerUpdated {
                server_id,
                name,
                icon_url,
            } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    if let Some(n) = name {
                        server.name = n;
                    }
                    if let Some(icon) = icon_url {
                        server.icon_url = Some(icon);
                    }
                }
            }

            ServerEvent::ServerDeleted { server_id } => {
                self.servers.retain(|s| s.id != server_id);
                if self.servers.is_empty() {
                    self.is_home = true;
                }
            }

            ServerEvent::ChannelCreated {
                server_id,
                category_id,
                channel,
            } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    if let Some(cat) = server.categories.iter_mut().find(|c| c.id == category_id) {
                        cat.channels.push(channel);
                    }
                }
            }

            ServerEvent::ChannelDeleted {
                server_id,
                channel_id,
            } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    for cat in &mut server.categories {
                        cat.channels.retain(|ch| ch.id != channel_id);
                    }
                }
                if self.active_channel_id == Some(channel_id) {
                    self.active_channel_id = None;
                }
            }

            ServerEvent::ChannelUpdated {
                channel_id,
                name,
                topic,
            } => {
                for server in &mut self.servers {
                    for cat in &mut server.categories {
                        if let Some(ch) = cat.channels.iter_mut().find(|c| c.id == channel_id) {
                            if let Some(n) = name.clone() {
                                ch.name = n;
                            }
                            if let Some(t) = topic.clone() {
                                ch.topic = Some(t);
                            }
                        }
                    }
                }
            }

            ServerEvent::CategoryCreated {
                server_id,
                category,
            } => {
                if let Some(server) = self.servers.iter_mut().find(|s| s.id == server_id) {
                    server.categories.push(category);
                }
            }

            ServerEvent::InviteCode {
                server_id,
                invite_code,
            } => {
                tracing::info!("Invite code for server {}: {}", server_id, invite_code);
            }

            ServerEvent::ProfileUpdated {
                user_id: uid,
                display_name,
                custom_status,
            } => {
                if let Some(member) = self.members.iter_mut().find(|m| m.user.id == uid) {
                    if let Some(dn) = display_name {
                        member.user.display_name = Some(dn);
                    }
                    if let Some(cs) = custom_status {
                        member.user.custom_status = Some(cs);
                    }
                }
            }

            ServerEvent::DirectMessageHistory {
                friend_id,
                messages,
                ..
            } => {
                let dm_channel_id = self.dm_channel_id(friend_id);
                for dm in messages {
                    let msg_id = dm.id;
                    if !self.messages.iter().any(|m| m.id == msg_id) {
                        let sent_by_us = Some(dm.sender_id) == self.user_id;
                        let content = self.dm_text(friend_id, sent_by_us, dm.encrypted_content);

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
                let server_strip =
                    server_list::view_from_payloads(&self.servers, active_srv_idx, self.is_home)
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

                    let reply_msg = self
                        .reply_to_id
                        .and_then(|rid| self.messages.iter().find(|m| m.id == rid));
                    let chat_view = chat_area::view_with_payload(
                        None,
                        &self.messages,
                        &self.message_input,
                        dm_channel,
                        reply_msg,
                        &self.search_query,
                        // The relay pins channel messages only.
                        false,
                        &self.outbox,
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

                    let reply_msg = self
                        .reply_to_id
                        .and_then(|rid| self.messages.iter().find(|m| m.id == rid));
                    let chat_view = chat_area::view_with_payload(
                        active_channel_payload,
                        &self.messages,
                        &self.message_input,
                        self.active_channel_id,
                        reply_msg,
                        &self.search_query,
                        self.can_pin(),
                        &self.outbox,
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

                let mut screen = column![];
                if self.offline() {
                    let offline = "Offline. Reconnecting to the relay...";
                    screen = screen.push(banner(offline, colors::STATUS_IDLE, None));
                }
                if let Some(error) = &self.banner {
                    screen = screen.push(banner(
                        error,
                        colors::DANGER,
                        Some(AppMessage::DismissBanner),
                    ));
                }
                container(screen.push(main_row))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        }
    }
}

/// A strip above the chat, with a close button when `dismiss` is given.
fn banner(message: &str, color: Color, dismiss: Option<AppMessage>) -> Element<'_, AppMessage> {
    let mut content = row![
        text(message).size(13).color(colors::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
    ]
    .align_y(Vertical::Center);
    if let Some(dismiss) = dismiss {
        content = content.push(
            button(text(icons::CLOSE).size(12).color(colors::TEXT_PRIMARY))
                .on_press(dismiss)
                .padding(Padding::from([2, 6]))
                .style(|_theme, _status| button::Style {
                    background: None,
                    text_color: colors::TEXT_PRIMARY,
                    ..button::Style::default()
                }),
        );
    }
    container(content)
        .padding(Padding::from([6, 16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..container::Style::default()
        })
        .into()
}

/// A server member as the member list shows it.
fn server_member(m: &MemberPayload) -> ServerMember {
    ServerMember {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artemis_core::limits::MAX_DM_CIPHERTEXT_BYTES;

    fn type_and_send(app: &mut Artemis, text: &str) {
        let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::InputChanged(
            text.to_string(),
        )));
        let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::SendMessage));
    }

    #[test]
    fn send_over_the_limit_does_nothing_and_keeps_the_text() {
        let (mut app, _) = Artemis::new();
        let (tx, mut sent) = mpsc::unbounded_channel();
        app.client_tx = Some(tx);
        app.is_home = false;
        app.active_channel_id = Some(Uuid::from_u128(1));

        // Counted in characters: each of these is two bytes.
        let over = "\u{e9}".repeat(MAX_MESSAGE_CHARS + 1);
        type_and_send(&mut app, &over);
        assert_eq!(app.message_input, over);
        assert!(sent.try_recv().is_err());

        let at_cap = "\u{e9}".repeat(MAX_MESSAGE_CHARS);
        type_and_send(&mut app, &at_cap);
        assert!(app.message_input.is_empty());
        assert!(matches!(
            sent.try_recv(),
            Ok(ClientEvent::SendMessage { content, .. }) if content == at_cap
        ));
    }

    const CHANNEL: Uuid = Uuid::from_u128(1);

    /// The app signed in with an open socket, in CHANNEL of its one server.
    fn connected() -> (Artemis, mpsc::UnboundedReceiver<ClientEvent>) {
        let (mut app, _) = Artemis::new();
        let (tx, sent) = mpsc::unbounded_channel();
        let _ = app.update(AppMessage::Connected(tx));
        app.user_id = Some(Uuid::from_u128(2));
        app.username = "ada".to_string();
        app.is_home = false;
        app.active_channel_id = Some(CHANNEL);
        (app, sent)
    }

    /// Sends `text` in the active channel and returns the nonce it went with.
    fn send_text(app: &mut Artemis, sent: &mut mpsc::UnboundedReceiver<ClientEvent>) -> Uuid {
        type_and_send(app, "hello");
        match sent.try_recv() {
            Ok(ClientEvent::SendMessage {
                client_nonce: Some(nonce),
                ..
            }) => nonce,
            other => panic!("expected SendMessage with a nonce, got {other:?}"),
        }
    }

    fn event(app: &mut Artemis, event: ServerEvent) {
        let _ = app.update(AppMessage::ServerEventReceived(event));
    }

    #[test]
    fn a_sent_message_shows_pending_and_its_echo_replaces_it_once() {
        let (mut app, mut sent) = connected();
        let nonce = send_text(&mut app, &mut sent);
        assert!(app.message_input.is_empty());
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].id, nonce);
        assert!(!app.outbox[&nonce].failed);

        let stored = Message {
            id: Uuid::from_u128(3),
            ..app.messages[0].clone()
        };
        let echo = ServerEvent::MessageReceived {
            message: stored.clone(),
            client_nonce: Some(nonce),
        };
        event(&mut app, echo.clone());
        event(&mut app, echo);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].id, stored.id);
        assert!(app.outbox.is_empty());
    }

    #[test]
    fn a_lost_connection_fails_pending_messages_and_keeps_new_input() {
        let (mut app, mut sent) = connected();
        let nonce = send_text(&mut app, &mut sent);

        let _ = app.update(AppMessage::Disconnected("reset".to_string()));
        assert!(app.offline());
        assert!(app.outbox[&nonce].failed);

        // Offline, the text and the reply stay in the composer.
        let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::ReplyTo(nonce)));
        type_and_send(&mut app, "still here");
        assert_eq!(app.message_input, "still here");
        assert_eq!(app.reply_to_id, Some(nonce));
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn retry_sends_the_same_event_again_once_reconnected() {
        let (mut app, mut sent) = connected();
        let nonce = send_text(&mut app, &mut sent);
        let _ = app.update(AppMessage::Disconnected("reset".to_string()));
        // Nothing goes out while offline.
        let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::Retry(nonce)));
        assert!(app.outbox[&nonce].failed);

        let (tx, mut resent) = mpsc::unbounded_channel();
        let _ = app.update(AppMessage::Connected(tx));
        let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::Retry(nonce)));
        assert!(matches!(
            resent.try_recv(),
            Ok(ClientEvent::SendMessage { content, client_nonce, .. })
                if content == "hello" && client_nonce == Some(nonce)
        ));
        assert!(!app.outbox[&nonce].failed);
        assert_eq!(app.messages.len(), 1);

        let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::Discard(nonce)));
        assert!(app.outbox.is_empty() && app.messages.is_empty());
    }

    #[test]
    fn a_relay_error_shows_a_banner_until_dismissed() {
        let (mut app, _sent) = connected();
        event(
            &mut app,
            ServerEvent::Error {
                message: "Not a member of this server".to_string(),
            },
        );
        assert_eq!(app.banner.as_deref(), Some("Not a member of this server"));
        let _ = app.update(AppMessage::DismissBanner);
        assert_eq!(app.banner, None);

        let _ = app.update(AppMessage::ConnectionFailed("refused".to_string()));
        assert_eq!(
            app.banner.as_deref(),
            Some("Cannot reach the relay: refused")
        );
        let (tx, _sent) = mpsc::unbounded_channel();
        let _ = app.update(AppMessage::Connected(tx));
        assert_eq!(app.banner, None);
    }

    #[test]
    fn an_auth_error_ends_the_session() {
        let (mut app, _sent) = connected();
        event(
            &mut app,
            ServerEvent::AuthError {
                reason: "Invalid or expired session".to_string(),
            },
        );
        assert!(app.client_tx.is_none());
        assert!(matches!(app.screen, AppScreen::Login));
        assert_eq!(
            app.login_state,
            LoginState::Error("Invalid or expired session".to_string())
        );
    }

    #[test]
    fn a_reconnection_keeps_the_channel_on_screen() {
        let (mut app, mut sent) = connected();
        let server = |id: u128, channels: &[u128]| ServerPayload {
            id: Uuid::from_u128(id),
            name: format!("server {id}"),
            icon_url: None,
            categories: vec![artemis_core::protocol::CategoryPayload {
                id: Uuid::from_u128(id + 100),
                name: "GENERAL".to_string(),
                channels: channels
                    .iter()
                    .map(|&ch| artemis_core::protocol::ChannelPayload {
                        id: Uuid::from_u128(ch),
                        name: format!("channel {ch}"),
                        channel_type: artemis_core::models::channel::ChannelType::Text,
                        topic: None,
                    })
                    .collect(),
            }],
            members: vec![],
        };
        let authenticated = |servers| ServerEvent::Authenticated {
            user_id: Uuid::from_u128(2),
            username: "ada".to_string(),
            servers,
        };
        event(
            &mut app,
            authenticated(vec![server(10, &[11]), server(20, &[21, 22])]),
        );
        let _ = app.update(AppMessage::ServerList(ServerListMsg::SelectServer(1)));
        let _ = app.update(AppMessage::ChannelSidebar(
            ChannelSidebarMsg::SelectChannel(Uuid::from_u128(22)),
        ));

        // Server 10 is gone after the reconnection, which moves server 20.
        event(&mut app, authenticated(vec![server(20, &[21, 22])]));
        assert_eq!(app.active_server_idx, 0);
        assert_eq!(app.active_channel_id, Some(Uuid::from_u128(22)));
        let fetched: Vec<ClientEvent> = std::iter::from_fn(|| sent.try_recv().ok()).collect();
        assert!(fetched.iter().any(|e| matches!(
            e,
            ClientEvent::FetchMessages { channel_id, .. } if *channel_id == Uuid::from_u128(22)
        )));
    }

    #[test]
    fn the_composer_drops_nul_and_cuts_a_huge_paste() {
        let (mut app, _) = Artemis::new();
        let input = |app: &mut Artemis, text: String| {
            let _ = app.update(AppMessage::ChatArea(ChatAreaMsg::InputChanged(text)));
        };
        input(&mut app, "a\0b".to_string());
        assert_eq!(app.message_input, "ab");
        assert!(chat_area::can_send(&app.message_input));

        input(&mut app, "\u{e9}".repeat(1_000_000));
        assert_eq!(app.message_input, "\u{e9}".repeat(2 * MAX_MESSAGE_CHARS));
    }

    #[test]
    fn the_longest_message_encrypts_within_the_relay_cap() {
        let (alice, bob) = (Identity::generate(), Identity::generate());
        // Each of these is four bytes, the most a character takes.
        let longest = "\u{1F600}".repeat(MAX_MESSAGE_CHARS);
        let sealed = alice
            .encrypt_for_b64(
                Purpose::DirectMessage,
                &bob.public_key_b64(),
                longest.as_bytes(),
            )
            .unwrap();
        assert_eq!(sealed.len(), MAX_DM_CIPHERTEXT_BYTES);
    }
}
