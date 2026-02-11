use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::message::Message;
use crate::models::user::{UserStatus, MemberRole};

/// Events sent FROM client TO server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientEvent {
    /// Authenticate with the server.
    Authenticate { token: String },

    /// Send a message to a channel.
    SendMessage {
        channel_id: Uuid,
        content: String,
    },

    /// Edit an existing message.
    EditMessage {
        message_id: Uuid,
        content: String,
    },

    /// Delete a message.
    DeleteMessage { message_id: Uuid },

    /// Start typing indicator.
    StartTyping { channel_id: Uuid },

    /// Join a server via invite code.
    JoinServer { invite_code: String },

    /// Request message history for a channel.
    FetchMessages {
        channel_id: Uuid,
        before: Option<Uuid>,
        limit: u32,
    },

    /// Update own presence status.
    UpdatePresence { status: UserStatus },

    /// Create a new channel in a server.
    CreateChannel {
        server_id: Uuid,
        name: String,
        category_id: Option<Uuid>,
    },

    // ── Friend / DM / E2E relay ──

    /// Publish own public key to the server.
    PublishPublicKey { public_key: String },

    /// Send a friend request by GitHub username.
    SendFriendRequest { target_username: String },

    /// Accept a pending friend request.
    AcceptFriendRequest { from_user_id: Uuid },

    /// Decline a pending friend request.
    DeclineFriendRequest { from_user_id: Uuid },

    /// Send an E2E encrypted DM relayed through the server.
    SendDirectMessage {
        recipient_id: Uuid,
        encrypted_content: String,
    },

    /// Fetch own friend list.
    FetchFriends,

    /// Fetch pending incoming friend requests.
    FetchFriendRequests,
}

/// Events sent FROM server TO client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerEvent {
    /// Authentication succeeded.
    Authenticated {
        user_id: Uuid,
        username: String,
        servers: Vec<ServerPayload>,
    },

    /// Authentication failed.
    AuthError { reason: String },

    /// A new message was received.
    MessageReceived { message: Message },

    /// A message was edited.
    MessageEdited {
        message_id: Uuid,
        content: String,
        edited_at: DateTime<Utc>,
    },

    /// A message was deleted.
    MessageDeleted { message_id: Uuid },

    /// Message history response.
    MessageHistory {
        channel_id: Uuid,
        messages: Vec<Message>,
        has_more: bool,
    },

    /// A user started typing.
    UserTyping {
        channel_id: Uuid,
        user_id: Uuid,
        username: String,
    },

    /// A user's presence changed.
    PresenceUpdate {
        user_id: Uuid,
        status: UserStatus,
    },

    /// A user joined the server.
    MemberJoined {
        server_id: Uuid,
        user_id: Uuid,
        username: String,
        avatar_url: Option<String>,
        role: MemberRole,
    },

    /// A user left the server.
    MemberLeft {
        server_id: Uuid,
        user_id: Uuid,
    },

    /// Server error.
    Error { message: String },

    // ── Friend / DM / E2E relay ──

    /// Own public key was saved.
    PublicKeyAcknowledged,

    /// Someone sent us a friend request.
    FriendRequestReceived {
        from_user_id: Uuid,
        from_username: String,
        from_avatar_url: Option<String>,
    },

    /// A friend request we sent was accepted.
    FriendRequestAccepted {
        user_id: Uuid,
        username: String,
        avatar_url: Option<String>,
        public_key: Option<String>,
    },

    /// A friend request was declined.
    FriendRequestDeclined { by_user_id: Uuid },

    /// Full friend list response.
    FriendList { friends: Vec<FriendPayload> },

    /// Pending friend requests list.
    PendingFriendRequests { requests: Vec<FriendRequestPayload> },

    /// An E2E encrypted DM relayed to us.
    DirectMessageReceived {
        from_user_id: Uuid,
        from_username: String,
        encrypted_content: String,
        timestamp: DateTime<Utc>,
        message_id: Uuid,
    },

    /// A friend's presence changed.
    FriendPresenceUpdate {
        user_id: Uuid,
        status: UserStatus,
    },
}

/// Compact server info sent on auth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerPayload {
    pub id: Uuid,
    pub name: String,
    pub icon_url: Option<String>,
    pub categories: Vec<CategoryPayload>,
    pub members: Vec<MemberPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryPayload {
    pub id: Uuid,
    pub name: String,
    pub channels: Vec<ChannelPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPayload {
    pub id: Uuid,
    pub name: String,
    pub channel_type: crate::models::channel::ChannelType,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberPayload {
    pub user_id: Uuid,
    pub username: String,
    pub avatar_url: Option<String>,
    pub role: MemberRole,
    pub status: UserStatus,
    pub custom_status: Option<String>,
}

/// A friend in the friend list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendPayload {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub public_key: Option<String>,
    pub status: UserStatus,
}

/// A pending friend request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendRequestPayload {
    pub from_user_id: Uuid,
    pub from_username: String,
    pub from_avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_event_roundtrip() {
        let event = ClientEvent::SendMessage {
            channel_id: Uuid::nil(),
            content: "hello".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ClientEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ClientEvent::SendMessage { channel_id, content } => {
                assert_eq!(channel_id, Uuid::nil());
                assert_eq!(content, "hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn server_event_roundtrip() {
        let event = ServerEvent::AuthError {
            reason: "invalid token".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ServerEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerEvent::AuthError { reason } => assert_eq!(reason, "invalid token"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn server_payload_serialization() {
        let payload = ServerPayload {
            id: Uuid::nil(),
            name: "Test Server".to_string(),
            icon_url: None,
            categories: vec![],
            members: vec![],
        };
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: ServerPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "Test Server");
        assert!(parsed.categories.is_empty());
    }

    #[test]
    fn user_status_default_is_offline() {
        assert_eq!(UserStatus::default(), UserStatus::Offline);
    }

    #[test]
    fn member_role_serialization() {
        let json = serde_json::to_string(&MemberRole::Founder).unwrap();
        assert_eq!(json, "\"Founder\"");
        let parsed: MemberRole = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, MemberRole::Founder);
    }
}
