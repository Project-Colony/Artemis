use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use artemis_core::models::user::UserStatus;

/// Messages exchanged directly between peers over QUIC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PeerMessage {
    /// Send a chat message.
    ChatMessage {
        id: Uuid,
        channel_id: Uuid,
        content: String,
        timestamp: DateTime<Utc>,
    },

    /// Edit a previously sent message.
    MessageEdit {
        message_id: Uuid,
        content: String,
        edited_at: DateTime<Utc>,
    },

    /// Delete a message.
    MessageDelete { message_id: Uuid },

    /// Typing indicator.
    Typing { channel_id: Uuid },

    /// Presence update.
    Presence { status: UserStatus },

    /// Request message history sync from peer.
    SyncRequest {
        channel_id: Uuid,
        before: Option<Uuid>,
        limit: u32,
    },

    /// Response with message history.
    SyncResponse {
        channel_id: Uuid,
        messages: Vec<artemis_core::models::message::Message>,
        has_more: bool,
    },

    /// Friend request (initial handshake after signaling).
    Hello {
        github_username: String,
        display_name: Option<String>,
        avatar_url: Option<String>,
        public_key: String,
    },

    /// Acknowledge friend connection.
    HelloAck {
        github_username: String,
        display_name: Option<String>,
        avatar_url: Option<String>,
    },

    /// Ping to keep connection alive.
    Ping { timestamp: DateTime<Utc> },

    /// Pong response.
    Pong { timestamp: DateTime<Utc> },
}

/// Connection info exchanged via GitHub Gist signaling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// GitHub username of the peer.
    pub github_username: String,
    /// Public QUIC endpoint (after STUN discovery).
    pub endpoint: String,
    /// X25519 public key (base64-encoded).
    pub public_key: String,
    /// Self-signed TLS certificate fingerprint (for QUIC verification).
    pub cert_fingerprint: String,
    /// Timestamp of this connection info.
    pub timestamp: DateTime<Utc>,
}

/// A friend/peer stored locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerProfile {
    /// GitHub username.
    pub github_username: String,
    /// Display name.
    pub display_name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
    /// X25519 public key (base64).
    pub public_key: String,
    /// Their signaling Gist ID (for sending connection requests).
    pub signaling_gist_id: Option<String>,
    /// Last known status.
    pub status: UserStatus,
    /// When they were added as a friend.
    pub added_at: DateTime<Utc>,
}

/// Data stored in the user's public profile Gist (`artemis-profile.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileGist {
    /// X25519 public key (base64).
    pub public_key: String,
    /// ID of the signaling (mailbox) Gist.
    pub signaling_gist_id: String,
    /// Artemis protocol version.
    pub version: String,
}

/// A signaling message posted to a peer's mailbox Gist.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignalingMessage {
    /// Connection request with encrypted endpoint info.
    ConnectRequest {
        from_username: String,
        /// ConnectionInfo encrypted with recipient's public key, base64-encoded.
        encrypted_info: String,
        timestamp: DateTime<Utc>,
    },

    /// Connection accepted (response).
    ConnectAccept {
        from_username: String,
        encrypted_info: String,
        timestamp: DateTime<Utc>,
    },

    /// Friend request.
    FriendRequest {
        from_username: String,
        public_key: String,
        display_name: Option<String>,
        avatar_url: Option<String>,
        timestamp: DateTime<Utc>,
    },

    /// Friend request accepted.
    FriendAccepted {
        from_username: String,
        public_key: String,
        timestamp: DateTime<Utc>,
    },
}
