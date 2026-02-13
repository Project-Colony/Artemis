use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author_id: Uuid,
    pub author_name: String,
    pub author_avatar: Option<String>,
    pub content: String,
    pub attachments: Vec<Attachment>,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    /// ID of the message being replied to (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub reply_to_id: Option<Uuid>,
    /// Whether this message is pinned in its channel.
    #[serde(default)]
    pub pinned: bool,
    /// Aggregated reaction counts on this message.
    #[serde(default)]
    pub reactions: Vec<ReactionCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Uuid,
    pub filename: String,
    pub url: String,
    pub content_type: String,
    pub size_bytes: u64,
}

/// Aggregated reaction on a message (emoji + count + whether current user reacted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionCount {
    pub emoji: String,
    pub count: u32,
    pub me: bool,
}
