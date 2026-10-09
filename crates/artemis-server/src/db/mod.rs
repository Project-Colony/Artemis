use chrono::{DateTime, Utc};
use sqlx::postgres::PgPool;
use uuid::Uuid;

use artemis_core::models::channel::ChannelType;
use artemis_core::models::message::Message;
use artemis_core::models::user::{MemberRole, UserStatus};
use artemis_core::protocol::*;

pub type DbPool = PgPool;

pub async fn connect(database_url: &str) -> Result<DbPool, sqlx::Error> {
    let pool = PgPool::connect(database_url).await?;
    tracing::info!("Connected to PostgreSQL");
    Ok(pool)
}

/// Every file in `migrations/`, each applied once and recorded in
/// `_sqlx_migrations`. sqlx checksums applied files, so a schema change is
/// always a new numbered file, never an edit to an existing one.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Readies the database before the relay opens its listener: applies pending
/// migrations, then sets every user offline. Nobody is connected yet, and a
/// relay that stopped without running its disconnect cleanup would otherwise
/// keep showing its last users online.
pub async fn prepare(pool: &DbPool) -> Result<(), sqlx::Error> {
    MIGRATOR.run(pool).await?;
    sqlx::query("UPDATE users SET status = 'offline' WHERE status <> 'offline'")
        .execute(pool)
        .await?;
    Ok(())
}

// ── User queries ──

pub async fn find_user_by_github_id(
    pool: &DbPool,
    github_id: i64,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE github_id = $1"
    )
    .bind(github_id)
    .fetch_optional(pool)
    .await
}

pub async fn find_user_by_token(
    pool: &DbPool,
    token: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE auth_token = $1"
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

pub async fn find_user_by_username(
    pool: &DbPool,
    username: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE username = $1"
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

pub async fn create_user(
    pool: &DbPool,
    username: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
    github_id: i64,
    auth_token: &str,
) -> Result<UserRow, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (username, display_name, avatar_url, github_id, auth_token, status)
         VALUES ($1, $2, $3, $4, $5, 'online')
         RETURNING id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, created_at"
    )
    .bind(username)
    .bind(display_name)
    .bind(avatar_url)
    .bind(github_id)
    .bind(auth_token)
    .fetch_one(pool)
    .await
}

pub async fn update_user_token(
    pool: &DbPool,
    user_id: Uuid,
    token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET auth_token = $1 WHERE id = $2")
        .bind(token)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_user_status(
    pool: &DbPool,
    user_id: Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET status = $1 WHERE id = $2")
        .bind(status)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Server queries ──

pub async fn get_user_servers(
    pool: &DbPool,
    user_id: Uuid,
) -> Result<Vec<ServerPayload>, sqlx::Error> {
    let server_rows = sqlx::query_as::<_, ServerRow>(
        "SELECT s.id, s.name, s.icon_url
         FROM servers s
         JOIN server_members sm ON sm.server_id = s.id
         WHERE sm.user_id = $1
         ORDER BY s.created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut payloads = Vec::new();
    for server in server_rows {
        let categories = get_server_categories(pool, server.id).await?;
        let members = get_server_members(pool, server.id).await?;
        payloads.push(ServerPayload {
            id: server.id,
            name: server.name,
            icon_url: server.icon_url,
            categories,
            members,
        });
    }
    Ok(payloads)
}

// Unused until clients can create a server over the WebSocket. The REST route
// that called it took the session token in the query string.
#[allow(dead_code)]
pub async fn create_server(
    pool: &DbPool,
    name: &str,
    owner_id: Uuid,
    invite_code: &str,
) -> Result<Uuid, sqlx::Error> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO servers (name, owner_id, invite_code) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .bind(invite_code)
    .fetch_one(pool)
    .await?;

    let server_id = row.0;

    // Add owner as founder member
    sqlx::query("INSERT INTO server_members (user_id, server_id, role) VALUES ($1, $2, 'founder')")
        .bind(owner_id)
        .bind(server_id)
        .execute(pool)
        .await?;

    // Create default category + channel
    let cat_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO categories (server_id, name, position) VALUES ($1, 'GENERAL', 0) RETURNING id",
    )
    .bind(server_id)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "INSERT INTO channels (category_id, server_id, name, channel_type, topic, position)
         VALUES ($1, $2, 'general', 'text', 'Welcome!', 0)",
    )
    .bind(cat_id.0)
    .bind(server_id)
    .execute(pool)
    .await?;

    Ok(server_id)
}

pub async fn join_server_by_invite(
    pool: &DbPool,
    user_id: Uuid,
    invite_code: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let server = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM servers WHERE invite_code = $1")
        .bind(invite_code)
        .fetch_optional(pool)
        .await?;

    if let Some((server_id,)) = server {
        sqlx::query(
            "INSERT INTO server_members (user_id, server_id, role) VALUES ($1, $2, 'member') ON CONFLICT DO NOTHING"
        )
        .bind(user_id)
        .bind(server_id)
        .execute(pool)
        .await?;
        Ok(Some(server_id))
    } else {
        Ok(None)
    }
}

async fn get_server_categories(
    pool: &DbPool,
    server_id: Uuid,
) -> Result<Vec<CategoryPayload>, sqlx::Error> {
    let cat_rows = sqlx::query_as::<_, CategoryRow>(
        "SELECT id, name FROM categories WHERE server_id = $1 ORDER BY position",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await?;

    let mut categories = Vec::new();
    for cat in cat_rows {
        let channels = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, channel_type, topic FROM channels WHERE category_id = $1 ORDER BY position"
        )
        .bind(cat.id)
        .fetch_all(pool)
        .await?;

        categories.push(CategoryPayload {
            id: cat.id,
            name: cat.name,
            channels: channels
                .into_iter()
                .map(|ch| ChannelPayload {
                    id: ch.id,
                    name: ch.name,
                    channel_type: parse_channel_type(&ch.channel_type),
                    topic: ch.topic,
                })
                .collect(),
        });
    }
    Ok(categories)
}

async fn get_server_members(
    pool: &DbPool,
    server_id: Uuid,
) -> Result<Vec<MemberPayload>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id, u.username, u.avatar_url, sm.role, u.status, u.custom_status
         FROM users u
         JOIN server_members sm ON sm.user_id = u.id
         WHERE sm.server_id = $1
         ORDER BY sm.role, u.username",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MemberPayload {
            user_id: r.id,
            username: r.username,
            avatar_url: r.avatar_url,
            role: parse_role(&r.role),
            status: parse_status(&r.status),
            custom_status: r.custom_status,
        })
        .collect())
}

// ── Message queries ──

pub async fn create_message(
    pool: &DbPool,
    channel_id: Uuid,
    author_id: Uuid,
    content: &str,
    reply_to_id: Option<Uuid>,
) -> Result<Message, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (channel_id, author_id, content, reply_to_id)
         VALUES ($1, $2, $3, $4)
         RETURNING id, channel_id, author_id, content, edited_at, created_at, reply_to_id",
    )
    .bind(channel_id)
    .bind(author_id)
    .bind(content)
    .bind(reply_to_id)
    .fetch_one(pool)
    .await?;

    let author = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT username, avatar_url FROM users WHERE id = $1",
    )
    .bind(author_id)
    .fetch_one(pool)
    .await?;

    Ok(Message {
        id: row.id,
        channel_id: row.channel_id,
        author_id: row.author_id,
        author_name: author.0,
        author_avatar: author.1,
        content: row.content,
        attachments: vec![],
        timestamp: row.created_at,
        edited_at: row.edited_at,
        reply_to_id: row.reply_to_id,
        pinned: false,
        reactions: vec![],
    })
}

pub async fn get_messages(
    pool: &DbPool,
    channel_id: Uuid,
    before: Option<Uuid>,
    limit: u32,
) -> Result<Vec<Message>, sqlx::Error> {
    let rows = if let Some(before_id) = before {
        sqlx::query_as::<_, MessageRow>(
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at, m.reply_to_id
             FROM messages m
             WHERE m.channel_id = $1 AND m.created_at < (SELECT created_at FROM messages WHERE id = $2)
             ORDER BY m.created_at DESC
             LIMIT $3"
        )
        .bind(channel_id)
        .bind(before_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, MessageRow>(
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at, m.reply_to_id
             FROM messages m
             WHERE m.channel_id = $1
             ORDER BY m.created_at DESC
             LIMIT $2"
        )
        .bind(channel_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await?
    };

    let mut messages = Vec::new();
    for row in rows {
        let author = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT username, avatar_url FROM users WHERE id = $1",
        )
        .bind(row.author_id)
        .fetch_one(pool)
        .await?;

        let pinned = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM pinned_messages WHERE message_id = $1",
        )
        .bind(row.id)
        .fetch_one(pool)
        .await
        .map(|r| r.0 > 0)
        .unwrap_or(false);

        messages.push(Message {
            id: row.id,
            channel_id: row.channel_id,
            author_id: row.author_id,
            author_name: author.0,
            author_avatar: author.1,
            content: row.content,
            attachments: vec![],
            timestamp: row.created_at,
            edited_at: row.edited_at,
            reply_to_id: row.reply_to_id,
            pinned,
            reactions: vec![],
        });
    }

    messages.reverse();
    Ok(messages)
}

pub async fn edit_message(
    pool: &DbPool,
    message_id: Uuid,
    author_id: Uuid,
    content: &str,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    let result = sqlx::query_as::<_, (DateTime<Utc>,)>(
        "UPDATE messages SET content = $1, edited_at = NOW()
         WHERE id = $2 AND author_id = $3
         RETURNING edited_at",
    )
    .bind(content)
    .bind(message_id)
    .bind(author_id)
    .fetch_optional(pool)
    .await?;

    Ok(result.map(|r| r.0))
}

pub async fn delete_message(
    pool: &DbPool,
    message_id: Uuid,
    author_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM messages WHERE id = $1 AND author_id = $2")
        .bind(message_id)
        .bind(author_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Adds a text channel to `category_id`, or to the server's first category
/// when none is given, and returns the category it went into with the channel.
/// Returns None, and inserts nothing, when the category is not in `server_id`.
pub async fn create_channel(
    pool: &DbPool,
    server_id: Uuid,
    category_id: Option<Uuid>,
    name: &str,
) -> Result<Option<(Uuid, ChannelPayload)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "INSERT INTO channels (category_id, server_id, name, channel_type, position)
         SELECT c.id, c.server_id, $3, 'text',
                (SELECT COALESCE(MAX(position), 0) + 1 FROM channels WHERE category_id = c.id)
         FROM categories c
         WHERE c.server_id = $1 AND ($2::uuid IS NULL OR c.id = $2)
         ORDER BY c.position
         LIMIT 1
         RETURNING id, category_id",
    )
    .bind(server_id)
    .bind(category_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id, category_id)| {
        (
            category_id,
            ChannelPayload {
                id,
                name: name.to_string(),
                channel_type: ChannelType::Text,
                topic: None,
            },
        )
    }))
}

// ── Row types ──

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub github_id: Option<i64>,
    pub status: String,
    pub custom_status: Option<String>,
    pub auth_token: Option<String>,
    pub public_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
pub struct FriendRequestRow {
    pub id: Uuid,
    pub from_user_id: Uuid,
    pub to_user_id: Uuid,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
pub struct FriendRow {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub public_key: Option<String>,
    pub status: String,
}

#[derive(sqlx::FromRow)]
pub struct FriendRequestDetailRow {
    pub from_user_id: Uuid,
    pub from_username: String,
    pub from_avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ServerRow {
    id: Uuid,
    name: String,
    icon_url: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CategoryRow {
    id: Uuid,
    name: String,
}

#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: Uuid,
    name: String,
    channel_type: String,
    topic: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    id: Uuid,
    username: String,
    avatar_url: Option<String>,
    role: String,
    status: String,
    custom_status: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Uuid,
    channel_id: Uuid,
    author_id: Uuid,
    content: String,
    edited_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    reply_to_id: Option<Uuid>,
}

// ── Public key ──

pub async fn save_public_key(
    pool: &DbPool,
    user_id: Uuid,
    public_key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET public_key = $1 WHERE id = $2")
        .bind(public_key)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Friend requests ──

pub async fn send_friend_request(
    pool: &DbPool,
    from_id: Uuid,
    to_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO friend_requests (from_user_id, to_user_id, status)
         VALUES ($1, $2, 'pending')
         ON CONFLICT (from_user_id, to_user_id) DO NOTHING",
    )
    .bind(from_id)
    .bind(to_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn accept_friend_request(
    pool: &DbPool,
    from_id: Uuid,
    to_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE friend_requests SET status = 'accepted'
         WHERE from_user_id = $1 AND to_user_id = $2 AND status = 'pending'",
    )
    .bind(from_id)
    .bind(to_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Ok(false);
    }

    // Create bidirectional friendship (smaller UUID first)
    let (a, b) = if from_id < to_id {
        (from_id, to_id)
    } else {
        (to_id, from_id)
    };
    sqlx::query("INSERT INTO friendships (user_a, user_b) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(a)
        .bind(b)
        .execute(pool)
        .await?;

    Ok(true)
}

pub async fn decline_friend_request(
    pool: &DbPool,
    from_id: Uuid,
    to_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE friend_requests SET status = 'declined'
         WHERE from_user_id = $1 AND to_user_id = $2 AND status = 'pending'",
    )
    .bind(from_id)
    .bind(to_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_pending_friend_requests(
    pool: &DbPool,
    user_id: Uuid,
) -> Result<Vec<FriendRequestDetailRow>, sqlx::Error> {
    sqlx::query_as::<_, FriendRequestDetailRow>(
        "SELECT fr.from_user_id, u.username AS from_username, u.avatar_url AS from_avatar_url, fr.created_at
         FROM friend_requests fr
         JOIN users u ON u.id = fr.from_user_id
         WHERE fr.to_user_id = $1 AND fr.status = 'pending'
         ORDER BY fr.created_at DESC"
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn are_friends(pool: &DbPool, user_a: Uuid, user_b: Uuid) -> Result<bool, sqlx::Error> {
    let (a, b) = if user_a < user_b {
        (user_a, user_b)
    } else {
        (user_b, user_a)
    };
    let row = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM friendships WHERE user_a = $1 AND user_b = $2",
    )
    .bind(a)
    .bind(b)
    .fetch_one(pool)
    .await?;
    Ok(row.0 > 0)
}

pub async fn get_friends(pool: &DbPool, user_id: Uuid) -> Result<Vec<FriendRow>, sqlx::Error> {
    sqlx::query_as::<_, FriendRow>(
        "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url, u.public_key, u.status
         FROM friendships f
         JOIN users u ON u.id = CASE WHEN f.user_a = $1 THEN f.user_b ELSE f.user_a END
         WHERE f.user_a = $1 OR f.user_b = $1
         ORDER BY u.username",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

// ── Direct messages ──

pub async fn store_direct_message(
    pool: &DbPool,
    sender_id: Uuid,
    recipient_id: Uuid,
    encrypted_content: &str,
) -> Result<(Uuid, DateTime<Utc>), sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        "INSERT INTO direct_messages (sender_id, recipient_id, encrypted_content)
         VALUES ($1, $2, $3) RETURNING id, created_at",
    )
    .bind(sender_id)
    .bind(recipient_id)
    .bind(encrypted_content)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// ── Parse helpers ──

fn parse_role(s: &str) -> MemberRole {
    match s {
        "founder" => MemberRole::Founder,
        "moderator" => MemberRole::Moderator,
        _ => MemberRole::Member,
    }
}

fn parse_status(s: &str) -> UserStatus {
    match s {
        "online" => UserStatus::Online,
        "idle" => UserStatus::Idle,
        "dnd" => UserStatus::DoNotDisturb,
        _ => UserStatus::Offline,
    }
}

fn parse_channel_type(s: &str) -> ChannelType {
    match s {
        "voice" => ChannelType::Voice,
        _ => ChannelType::Text,
    }
}

// ── Reactions ──

pub async fn add_reaction(
    pool: &DbPool,
    message_id: Uuid,
    user_id: Uuid,
    emoji: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO message_reactions (message_id, user_id, emoji) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
    )
    .bind(message_id)
    .bind(user_id)
    .bind(emoji)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn remove_reaction(
    pool: &DbPool,
    message_id: Uuid,
    user_id: Uuid,
    emoji: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM message_reactions WHERE message_id = $1 AND user_id = $2 AND emoji = $3",
    )
    .bind(message_id)
    .bind(user_id)
    .bind(emoji)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

// ── Pins ──

pub async fn pin_message(
    pool: &DbPool,
    channel_id: Uuid,
    message_id: Uuid,
    pinned_by: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO pinned_messages (channel_id, message_id, pinned_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
    )
    .bind(channel_id)
    .bind(message_id)
    .bind(pinned_by)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn unpin_message(
    pool: &DbPool,
    channel_id: Uuid,
    message_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("DELETE FROM pinned_messages WHERE channel_id = $1 AND message_id = $2")
            .bind(channel_id)
            .bind(message_id)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_pinned_messages(
    pool: &DbPool,
    channel_id: Uuid,
) -> Result<Vec<Message>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MessageRow>(
        "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at, m.reply_to_id
         FROM pinned_messages pm
         JOIN messages m ON m.id = pm.message_id
         WHERE pm.channel_id = $1
         ORDER BY pm.pinned_at DESC"
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;

    let mut messages = Vec::new();
    for row in rows {
        let author = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT username, avatar_url FROM users WHERE id = $1",
        )
        .bind(row.author_id)
        .fetch_one(pool)
        .await?;

        messages.push(Message {
            id: row.id,
            channel_id: row.channel_id,
            author_id: row.author_id,
            author_name: author.0,
            author_avatar: author.1,
            content: row.content,
            attachments: vec![],
            timestamp: row.created_at,
            edited_at: row.edited_at,
            reply_to_id: row.reply_to_id,
            pinned: true,
            reactions: vec![],
        });
    }
    Ok(messages)
}

/// The channel a message is in and that channel's server, as
/// `(channel_id, server_id)`.
pub async fn message_channel(
    pool: &DbPool,
    message_id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.channel_id, c.server_id
         FROM messages m
         JOIN channels c ON c.id = m.channel_id
         WHERE m.id = $1",
    )
    .bind(message_id)
    .fetch_optional(pool)
    .await
}

// ── Unread tracking ──

pub async fn ack_message(
    pool: &DbPool,
    user_id: Uuid,
    channel_id: Uuid,
    message_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO channel_read_state (user_id, channel_id, last_read_message_id, last_read_at, mention_count)
         VALUES ($1, $2, $3, NOW(), 0)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read_message_id = $3, last_read_at = NOW(), mention_count = 0"
    )
    .bind(user_id)
    .bind(channel_id)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_unread_state(
    pool: &DbPool,
    user_id: Uuid,
) -> Result<Vec<ChannelUnreadPayload>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UnreadRow>(
        "SELECT sm.server_id, c.id AS channel_id,
                rs.last_read_message_id,
                COALESCE(rs.mention_count, 0) AS mention_count,
                (SELECT COUNT(*) FROM messages m
                 WHERE m.channel_id = c.id
                 AND (rs.last_read_message_id IS NULL OR m.created_at > (SELECT created_at FROM messages WHERE id = rs.last_read_message_id))
                )::INTEGER AS unread_count
         FROM server_members sm
         JOIN channels c ON c.server_id = sm.server_id
         LEFT JOIN channel_read_state rs ON rs.user_id = $1 AND rs.channel_id = c.id
         WHERE sm.user_id = $1"
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChannelUnreadPayload {
            channel_id: r.channel_id,
            last_read_message_id: r.last_read_message_id,
            mention_count: r.mention_count as u32,
            unread_count: r.unread_count as u32,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
struct UnreadRow {
    server_id: Uuid,
    channel_id: Uuid,
    last_read_message_id: Option<Uuid>,
    mention_count: i32,
    unread_count: i32,
}

// ── Server / channel management ──

pub async fn edit_server(
    pool: &DbPool,
    server_id: Uuid,
    name: Option<&str>,
    icon_url: Option<&str>,
) -> Result<(), sqlx::Error> {
    if let Some(name) = name {
        sqlx::query("UPDATE servers SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(server_id)
            .execute(pool)
            .await?;
    }
    if let Some(icon) = icon_url {
        sqlx::query("UPDATE servers SET icon_url = $1 WHERE id = $2")
            .bind(icon)
            .bind(server_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn delete_server(pool: &DbPool, server_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM servers WHERE id = $1")
        .bind(server_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_server_owner(pool: &DbPool, server_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid,)>("SELECT owner_id FROM servers WHERE id = $1")
        .bind(server_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn get_member_role(
    pool: &DbPool,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT role FROM server_members WHERE user_id = $1 AND server_id = $2",
    )
    .bind(user_id)
    .bind(server_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn create_category(
    pool: &DbPool,
    server_id: Uuid,
    name: &str,
) -> Result<CategoryPayload, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO categories (server_id, name, position)
         VALUES ($1, $2, (SELECT COALESCE(MAX(position), 0) + 1 FROM categories WHERE server_id = $1))
         RETURNING id"
    )
    .bind(server_id)
    .bind(name)
    .fetch_one(pool)
    .await?;

    Ok(CategoryPayload {
        id: row.0,
        name: name.to_string(),
        channels: vec![],
    })
}

pub async fn delete_channel(pool: &DbPool, channel_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    // Return server_id before deleting
    let row = sqlx::query_as::<_, (Uuid,)>("SELECT server_id FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(pool)
        .await?;

    if let Some((server_id,)) = row {
        sqlx::query("DELETE FROM channels WHERE id = $1")
            .bind(channel_id)
            .execute(pool)
            .await?;
        Ok(Some(server_id))
    } else {
        Ok(None)
    }
}

pub async fn edit_channel(
    pool: &DbPool,
    channel_id: Uuid,
    name: Option<&str>,
    topic: Option<&str>,
) -> Result<(), sqlx::Error> {
    if let Some(name) = name {
        sqlx::query("UPDATE channels SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(channel_id)
            .execute(pool)
            .await?;
    }
    if let Some(topic) = topic {
        sqlx::query("UPDATE channels SET topic = $1 WHERE id = $2")
            .bind(topic)
            .bind(channel_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn channel_server(pool: &DbPool, channel_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid,)>("SELECT server_id FROM channels WHERE id = $1")
        .bind(channel_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn leave_server(
    pool: &DbPool,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM server_members WHERE user_id = $1 AND server_id = $2")
        .bind(user_id)
        .bind(server_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_invite_code(
    pool: &DbPool,
    server_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row =
        sqlx::query_as::<_, (Option<String>,)>("SELECT invite_code FROM servers WHERE id = $1")
            .bind(server_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|r| r.0))
}

// ── User profile ──

pub async fn update_profile(
    pool: &DbPool,
    user_id: Uuid,
    display_name: Option<&str>,
    custom_status: Option<&str>,
) -> Result<(), sqlx::Error> {
    if let Some(dn) = display_name {
        sqlx::query("UPDATE users SET display_name = $1 WHERE id = $2")
            .bind(dn)
            .bind(user_id)
            .execute(pool)
            .await?;
    }
    if let Some(cs) = custom_status {
        sqlx::query("UPDATE users SET custom_status = $1 WHERE id = $2")
            .bind(cs)
            .bind(user_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ── DM history ──

pub async fn get_direct_messages(
    pool: &DbPool,
    user_id: Uuid,
    friend_id: Uuid,
    before: Option<Uuid>,
    limit: u32,
) -> Result<Vec<DirectMessagePayload>, sqlx::Error> {
    let rows = if let Some(before_id) = before {
        sqlx::query_as::<_, DmRow>(
            "SELECT dm.id, dm.sender_id, u.username AS sender_username, dm.encrypted_content, dm.created_at
             FROM direct_messages dm
             JOIN users u ON u.id = dm.sender_id
             WHERE ((dm.sender_id = $1 AND dm.recipient_id = $2) OR (dm.sender_id = $2 AND dm.recipient_id = $1))
               AND dm.created_at < (SELECT created_at FROM direct_messages WHERE id = $3)
             ORDER BY dm.created_at DESC
             LIMIT $4"
        )
        .bind(user_id).bind(friend_id).bind(before_id).bind(limit as i64)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, DmRow>(
            "SELECT dm.id, dm.sender_id, u.username AS sender_username, dm.encrypted_content, dm.created_at
             FROM direct_messages dm
             JOIN users u ON u.id = dm.sender_id
             WHERE (dm.sender_id = $1 AND dm.recipient_id = $2) OR (dm.sender_id = $2 AND dm.recipient_id = $1)
             ORDER BY dm.created_at DESC
             LIMIT $3"
        )
        .bind(user_id).bind(friend_id).bind(limit as i64)
        .fetch_all(pool)
        .await?
    };

    let mut messages: Vec<DirectMessagePayload> = rows
        .into_iter()
        .map(|r| DirectMessagePayload {
            id: r.id,
            sender_id: r.sender_id,
            sender_username: r.sender_username,
            encrypted_content: r.encrypted_content,
            created_at: r.created_at,
        })
        .collect();

    messages.reverse();
    Ok(messages)
}

#[derive(sqlx::FromRow)]
struct DmRow {
    id: Uuid,
    sender_id: Uuid,
    sender_username: String,
    encrypted_content: String,
    created_at: DateTime<Utc>,
}

// ── Full-text search ──

pub async fn search_messages(
    pool: &DbPool,
    server_id: Uuid,
    channel_id: Option<Uuid>,
    query: &str,
    limit: u32,
) -> Result<Vec<Message>, sqlx::Error> {
    let rows = if let Some(ch_id) = channel_id {
        sqlx::query_as::<_, MessageRow>(
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at, m.reply_to_id
             FROM messages m
             WHERE m.channel_id = $1
               AND to_tsvector('english', m.content) @@ plainto_tsquery('english', $2)
             ORDER BY ts_rank(to_tsvector('english', m.content), plainto_tsquery('english', $2)) DESC
             LIMIT $3"
        )
        .bind(ch_id)
        .bind(query)
        .bind(limit as i64)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, MessageRow>(
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at, m.reply_to_id
             FROM messages m
             JOIN channels c ON c.id = m.channel_id
             WHERE c.server_id = $1
               AND to_tsvector('english', m.content) @@ plainto_tsquery('english', $2)
             ORDER BY ts_rank(to_tsvector('english', m.content), plainto_tsquery('english', $2)) DESC
             LIMIT $3"
        )
        .bind(server_id)
        .bind(query)
        .bind(limit as i64)
        .fetch_all(pool)
        .await?
    };

    let mut messages = Vec::new();
    for row in rows {
        let author = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT username, avatar_url FROM users WHERE id = $1",
        )
        .bind(row.author_id)
        .fetch_one(pool)
        .await?;

        messages.push(Message {
            id: row.id,
            channel_id: row.channel_id,
            author_id: row.author_id,
            author_name: author.0,
            author_avatar: author.1,
            content: row.content,
            attachments: vec![],
            timestamp: row.created_at,
            edited_at: row.edited_at,
            reply_to_id: row.reply_to_id,
            pinned: false,
            reactions: vec![],
        });
    }
    Ok(messages)
}

pub async fn search_messages_count(
    pool: &DbPool,
    server_id: Uuid,
    channel_id: Option<Uuid>,
    query: &str,
) -> Result<u32, sqlx::Error> {
    let row = if let Some(ch_id) = channel_id {
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM messages m
             WHERE m.channel_id = $1
               AND to_tsvector('english', m.content) @@ plainto_tsquery('english', $2)",
        )
        .bind(ch_id)
        .bind(query)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM messages m
             JOIN channels c ON c.id = m.channel_id
             WHERE c.server_id = $1
               AND to_tsvector('english', m.content) @@ plainto_tsquery('english', $2)",
        )
        .bind(server_id)
        .bind(query)
        .fetch_one(pool)
        .await?
    };
    Ok(row.0 as u32)
}

// ── Custom emojis ──

pub async fn add_custom_emoji(
    pool: &DbPool,
    server_id: Uuid,
    name: &str,
    image_url: &str,
    uploaded_by: Uuid,
) -> Result<CustomEmojiPayload, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO custom_emojis (server_id, name, image_url, uploaded_by)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(server_id)
    .bind(name)
    .bind(image_url)
    .bind(uploaded_by)
    .fetch_one(pool)
    .await?;

    Ok(CustomEmojiPayload {
        id: row.0,
        name: name.to_string(),
        image_url: image_url.to_string(),
        uploaded_by,
    })
}

pub async fn remove_custom_emoji(
    pool: &DbPool,
    server_id: Uuid,
    emoji_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM custom_emojis WHERE id = $1 AND server_id = $2")
        .bind(emoji_id)
        .bind(server_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_custom_emojis(
    pool: &DbPool,
    server_id: Uuid,
) -> Result<Vec<CustomEmojiPayload>, sqlx::Error> {
    let rows = sqlx::query_as::<_, CustomEmojiRow>(
        "SELECT id, name, image_url, uploaded_by FROM custom_emojis WHERE server_id = $1 ORDER BY name"
    )
    .bind(server_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| CustomEmojiPayload {
            id: r.id,
            name: r.name,
            image_url: r.image_url,
            uploaded_by: r.uploaded_by,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct CustomEmojiRow {
    id: Uuid,
    name: String,
    image_url: String,
    uploaded_by: Uuid,
}

// ── Mention tracking ──

pub async fn increment_mention_count(
    pool: &DbPool,
    user_id: Uuid,
    channel_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO channel_read_state (user_id, channel_id, last_read_message_id, last_read_at, mention_count)
         VALUES ($1, $2, NULL, NOW(), 1)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET mention_count = channel_read_state.mention_count + 1"
    )
    .bind(user_id)
    .bind(channel_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ── Attachments ──

#[allow(dead_code)]
pub async fn add_attachment(
    pool: &DbPool,
    message_id: Uuid,
    filename: &str,
    url: &str,
    content_type: &str,
    size_bytes: i64,
) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO attachments (message_id, filename, url, content_type, size_bytes)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(message_id)
    .bind(filename)
    .bind(url)
    .bind(content_type)
    .bind(size_bytes)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

#[allow(dead_code)]
pub async fn get_message_attachments(
    pool: &DbPool,
    message_id: Uuid,
) -> Result<Vec<artemis_core::models::message::Attachment>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, filename, url, content_type, size_bytes FROM attachments WHERE message_id = $1 ORDER BY uploaded_at"
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| artemis_core::models::message::Attachment {
            id: r.id,
            filename: r.filename,
            url: r.url,
            content_type: r.content_type,
            size_bytes: r.size_bytes as u64,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
struct AttachmentRow {
    id: Uuid,
    filename: String,
    url: String,
    content_type: String,
    size_bytes: i64,
}

/// Get user IDs for server members by username (for @mention resolution).
pub async fn resolve_mentions(
    pool: &DbPool,
    server_id: Uuid,
    usernames: &[String],
) -> Result<Vec<(String, Uuid)>, sqlx::Error> {
    let mut results = Vec::new();
    for username in usernames {
        let row = sqlx::query_as::<_, (Uuid,)>(
            "SELECT u.id FROM users u
             JOIN server_members sm ON sm.user_id = u.id
             WHERE sm.server_id = $1 AND u.username = $2",
        )
        .bind(server_id)
        .bind(username)
        .fetch_optional(pool)
        .await?;
        if let Some((uid,)) = row {
            results.push((username.clone(), uid));
        }
    }
    Ok(results)
}

// The ignored tests need a Postgres server: set DATABASE_URL and run
// `cargo test -p artemis-server -- --include-ignored`. sqlx::test gives each
// test its own empty database. CI runs them in the relay-db job.
#[cfg(test)]
mod tests {
    use super::*;

    async fn recorded_migrations(pool: &DbPool) -> usize {
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(pool)
            .await
            .unwrap();
        rows as usize
    }

    #[test]
    fn applied_migrations_are_unchanged() {
        // SHA-384 of each file as shipped. Deployed relays recorded these, and
        // an edited file stops them at startup with VersionMismatch.
        let frozen = [
            (1, "094404878e8a015a4798f1e22487f9fbe9c344c647c27423fd78448669e4a512c08d89efabe3ca64fad34343dcd58599"),
            (2, "d022f78b6bb03d6b61718df629fd94010bb9499a28805751272f180c5e57c58bb3bd6f06fe2069c3888c17abf9ba9c20"),
            (3, "4d64e9c20652479599559c08ce106762bb2c86661cce7da9981882c5ae87487c09634cd060c7a2306424989c927d4d3e"),
            (4, "9abcb7c84c0c61a8e77dfecec6819cd70409f6615579a2de5b6bcf014863809a9d5af6a74fa838730baa2c126de3cb72"),
        ];
        for (version, expected) in frozen {
            let migration = MIGRATOR.iter().find(|m| m.version == version).unwrap();
            let actual: String = migration
                .checksum
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(
                actual, expected,
                "migration {version} changed after release; revert it and put the change in a new numbered file"
            );
        }
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "needs DATABASE_URL"]
    async fn migrator_adopts_a_database_built_by_the_old_startup(pool: DbPool) {
        // Before the migrator, every start ran 001 to 004 with raw_sql and
        // recorded nothing, so those databases have no _sqlx_migrations table.
        for migration in MIGRATOR.iter().filter(|m| m.version <= 4) {
            sqlx::raw_sql(migration.sql.clone())
                .execute(&pool)
                .await
                .unwrap();
        }

        MIGRATOR.run(&pool).await.unwrap();
        MIGRATOR.run(&pool).await.unwrap();

        assert_eq!(recorded_migrations(&pool).await, MIGRATOR.iter().count());
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "needs DATABASE_URL"]
    async fn migrator_runs_twice_on_an_empty_database(pool: DbPool) {
        MIGRATOR.run(&pool).await.unwrap();
        MIGRATOR.run(&pool).await.unwrap();

        assert_eq!(recorded_migrations(&pool).await, MIGRATOR.iter().count());
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    #[ignore = "needs DATABASE_URL"]
    async fn startup_sets_users_left_online_offline(pool: DbPool) {
        sqlx::query("INSERT INTO users (username, github_id, status) VALUES ('ada', 1, 'online')")
            .execute(&pool)
            .await
            .unwrap();

        prepare(&pool).await.unwrap();

        let user = find_user_by_github_id(&pool, 1).await.unwrap().unwrap();
        assert_eq!(user.status, "offline");
    }
}
