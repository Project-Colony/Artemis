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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Uuid,
    pub filename: String,
    pub url: String,
    pub content_type: String,
    pub size_bytes: u64,
}
