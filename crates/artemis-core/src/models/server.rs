use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::Category;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: Uuid,
    pub name: String,
    pub icon_url: Option<String>,
    pub owner_id: Uuid,
    pub categories: Vec<Category>,
    pub created_at: DateTime<Utc>,
}
