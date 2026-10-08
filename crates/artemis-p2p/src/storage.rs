use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::protocol::PeerProfile;
use artemis_core::models::message::Message;
use artemis_core::models::user::UserStatus;

/// Local SQLite storage for messages, friends, and identity.
pub struct LocalStore {
    conn: Connection,
}

impl LocalStore {
    /// Open or create the local database at the given path.
    pub fn open(path: &str) -> Result<Self, StorageError> {
        let conn = Connection::open(path).map_err(|e| StorageError::Open(e.to_string()))?;
        let store = Self { conn };
        store.run_migrations()?;
        Ok(store)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory().map_err(|e| StorageError::Open(e.to_string()))?;
        let store = Self { conn };
        store.run_migrations()?;
        Ok(store)
    }

    fn run_migrations(&self) -> Result<(), StorageError> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS identity (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                github_username TEXT NOT NULL,
                github_token TEXT NOT NULL,
                secret_key_b64 TEXT NOT NULL,
                public_key_b64 TEXT NOT NULL,
                profile_gist_id TEXT,
                signaling_gist_id TEXT,
                avatar_url TEXT,
                display_name TEXT
            );

            CREATE TABLE IF NOT EXISTS friends (
                github_username TEXT PRIMARY KEY,
                display_name TEXT,
                avatar_url TEXT,
                public_key TEXT NOT NULL,
                signaling_gist_id TEXT,
                status TEXT NOT NULL DEFAULT 'Offline',
                added_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                author_id TEXT NOT NULL,
                author_name TEXT NOT NULL,
                author_avatar TEXT,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                edited_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id, timestamp);

            CREATE TABLE IF NOT EXISTS channels (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                peer_username TEXT,
                channel_type TEXT NOT NULL DEFAULT 'dm'
            );
            ",
            )
            .map_err(|e| StorageError::Migration(e.to_string()))?;

        Ok(())
    }

    // ── Identity ──

    /// Save the local user's identity.
    pub fn save_identity(
        &self,
        github_username: &str,
        github_token: &str,
        secret_key_b64: &str,
        public_key_b64: &str,
        avatar_url: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<(), StorageError> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO identity (id, github_username, github_token, secret_key_b64, public_key_b64, avatar_url, display_name)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
                params![github_username, github_token, secret_key_b64, public_key_b64, avatar_url, display_name],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;
        Ok(())
    }

    /// Load the local user's identity.
    pub fn load_identity(&self) -> Result<Option<StoredIdentity>, StorageError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT github_username, github_token, secret_key_b64, public_key_b64,
                        profile_gist_id, signaling_gist_id, avatar_url, display_name
                 FROM identity WHERE id = 1",
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;

        let result = stmt
            .query_row([], |row| {
                Ok(StoredIdentity {
                    github_username: row.get(0)?,
                    github_token: row.get(1)?,
                    secret_key_b64: row.get(2)?,
                    public_key_b64: row.get(3)?,
                    profile_gist_id: row.get(4)?,
                    signaling_gist_id: row.get(5)?,
                    avatar_url: row.get(6)?,
                    display_name: row.get(7)?,
                })
            })
            .ok();

        Ok(result)
    }

    /// Update the Gist IDs after signaling initialization.
    pub fn update_gist_ids(
        &self,
        profile_gist_id: &str,
        signaling_gist_id: &str,
    ) -> Result<(), StorageError> {
        self.conn
            .execute(
                "UPDATE identity SET profile_gist_id = ?1, signaling_gist_id = ?2 WHERE id = 1",
                params![profile_gist_id, signaling_gist_id],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;
        Ok(())
    }

    // ── Friends ──

    /// Add or update a friend.
    pub fn save_friend(&self, friend: &PeerProfile) -> Result<(), StorageError> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO friends (github_username, display_name, avatar_url, public_key, signaling_gist_id, status, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    friend.github_username,
                    friend.display_name,
                    friend.avatar_url,
                    friend.public_key,
                    friend.signaling_gist_id,
                    format!("{:?}", friend.status),
                    friend.added_at.to_rfc3339(),
                ],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;
        Ok(())
    }

    /// Remove a friend.
    pub fn remove_friend(&self, github_username: &str) -> Result<(), StorageError> {
        self.conn
            .execute(
                "DELETE FROM friends WHERE github_username = ?1",
                params![github_username],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;
        Ok(())
    }

    /// List all friends.
    pub fn list_friends(&self) -> Result<Vec<PeerProfile>, StorageError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT github_username, display_name, avatar_url, public_key, signaling_gist_id, status, added_at
                 FROM friends ORDER BY github_username",
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;

        let friends = stmt
            .query_map([], |row| {
                let status_str: String = row.get(5)?;
                let status = match status_str.as_str() {
                    "Online" => UserStatus::Online,
                    "Idle" => UserStatus::Idle,
                    "DoNotDisturb" => UserStatus::DoNotDisturb,
                    _ => UserStatus::Offline,
                };
                let added_str: String = row.get(6)?;
                let added_at = DateTime::parse_from_rfc3339(&added_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                Ok(PeerProfile {
                    github_username: row.get(0)?,
                    display_name: row.get(1)?,
                    avatar_url: row.get(2)?,
                    public_key: row.get(3)?,
                    signaling_gist_id: row.get(4)?,
                    status,
                    added_at,
                })
            })
            .map_err(|e| StorageError::Query(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StorageError::Query(e.to_string()))?;

        Ok(friends)
    }

    // ── Messages ──

    /// Save a message.
    pub fn save_message(&self, msg: &Message) -> Result<(), StorageError> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO messages (id, channel_id, author_id, author_name, author_avatar, content, timestamp, edited_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    msg.id.to_string(),
                    msg.channel_id.to_string(),
                    msg.author_id.to_string(),
                    msg.author_name,
                    msg.author_avatar,
                    msg.content,
                    msg.timestamp.to_rfc3339(),
                    msg.edited_at.map(|t| t.to_rfc3339()),
                ],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;
        Ok(())
    }

    /// Get messages for a channel, ordered by timestamp.
    pub fn get_messages(
        &self,
        channel_id: Uuid,
        limit: u32,
        before: Option<Uuid>,
    ) -> Result<Vec<Message>, StorageError> {
        let channel_str = channel_id.to_string();

        if let Some(before_id) = before {
            let before_str = before_id.to_string();
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, channel_id, author_id, author_name, author_avatar, content, timestamp, edited_at
                     FROM messages
                     WHERE channel_id = ?1 AND timestamp < (SELECT timestamp FROM messages WHERE id = ?2)
                     ORDER BY timestamp DESC
                     LIMIT ?3",
                )
                .map_err(|e| StorageError::Query(e.to_string()))?;

            let messages = stmt
                .query_map(params![channel_str, before_str, limit], |row| {
                    Self::row_to_message(row)
                })
                .map_err(|e| StorageError::Query(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| StorageError::Query(e.to_string()))?;

            Ok(messages)
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, channel_id, author_id, author_name, author_avatar, content, timestamp, edited_at
                     FROM messages
                     WHERE channel_id = ?1
                     ORDER BY timestamp DESC
                     LIMIT ?2",
                )
                .map_err(|e| StorageError::Query(e.to_string()))?;

            let messages = stmt
                .query_map(params![channel_str, limit], Self::row_to_message)
                .map_err(|e| StorageError::Query(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| StorageError::Query(e.to_string()))?;

            Ok(messages)
        }
    }

    /// Delete a message.
    pub fn delete_message(&self, message_id: Uuid) -> Result<(), StorageError> {
        self.conn
            .execute(
                "DELETE FROM messages WHERE id = ?1",
                params![message_id.to_string()],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;
        Ok(())
    }

    // ── Channels ──

    /// Create or get a DM channel with a peer.
    pub fn get_or_create_dm_channel(&self, peer_username: &str) -> Result<Uuid, StorageError> {
        // Check if DM channel already exists
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM channels WHERE peer_username = ?1 AND channel_type = 'dm'")
            .map_err(|e| StorageError::Query(e.to_string()))?;

        if let Ok(id_str) = stmt.query_row(params![peer_username], |row| {
            let s: String = row.get(0)?;
            Ok(s)
        }) {
            return Uuid::parse_str(&id_str).map_err(|e| StorageError::Query(e.to_string()));
        }

        // Create new DM channel
        let id = Uuid::new_v4();
        self.conn
            .execute(
                "INSERT INTO channels (id, name, peer_username, channel_type) VALUES (?1, ?2, ?3, 'dm')",
                params![id.to_string(), format!("DM: {}", peer_username), peer_username],
            )
            .map_err(|e| StorageError::Query(e.to_string()))?;

        Ok(id)
    }

    fn row_to_message(row: &rusqlite::Row) -> Result<Message, rusqlite::Error> {
        let id_str: String = row.get(0)?;
        let channel_str: String = row.get(1)?;
        let author_str: String = row.get(2)?;
        let ts_str: String = row.get(6)?;
        let edited_str: Option<String> = row.get(7)?;

        Ok(Message {
            id: Uuid::parse_str(&id_str).unwrap_or(Uuid::nil()),
            channel_id: Uuid::parse_str(&channel_str).unwrap_or(Uuid::nil()),
            author_id: Uuid::parse_str(&author_str).unwrap_or(Uuid::nil()),
            author_name: row.get(3)?,
            author_avatar: row.get(4)?,
            content: row.get(5)?,
            attachments: vec![],
            timestamp: DateTime::parse_from_rfc3339(&ts_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            edited_at: edited_str.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }),
            reply_to_id: None,
            pinned: false,
            reactions: vec![],
        })
    }
}

/// Stored identity data.
#[derive(Debug, Clone)]
pub struct StoredIdentity {
    pub github_username: String,
    pub github_token: String,
    pub secret_key_b64: String,
    pub public_key_b64: String,
    pub profile_gist_id: Option<String>,
    pub signaling_gist_id: Option<String>,
    pub avatar_url: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("failed to open database: {0}")]
    Open(String),
    #[error("migration failed: {0}")]
    Migration(String),
    #[error("query error: {0}")]
    Query(String),
}
