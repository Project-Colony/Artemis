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

pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    let migration = include_str!("../../migrations/001_init.sql");
    sqlx::raw_sql(migration).execute(pool).await?;
    tracing::info!("Database migrations applied");
    Ok(())
}

// ── User queries ──

pub async fn find_user_by_github_id(pool: &DbPool, github_id: i64) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, created_at FROM users WHERE github_id = $1"
    )
    .bind(github_id)
    .fetch_optional(pool)
    .await
}

pub async fn find_user_by_token(pool: &DbPool, token: &str) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, created_at FROM users WHERE auth_token = $1"
    )
    .bind(token)
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

pub async fn update_user_token(pool: &DbPool, user_id: Uuid, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET auth_token = $1 WHERE id = $2")
        .bind(token)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_user_status(pool: &DbPool, user_id: Uuid, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET status = $1 WHERE id = $2")
        .bind(status)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Server queries ──

pub async fn get_user_servers(pool: &DbPool, user_id: Uuid) -> Result<Vec<ServerPayload>, sqlx::Error> {
    let server_rows = sqlx::query_as::<_, ServerRow>(
        "SELECT s.id, s.name, s.icon_url, s.owner_id
         FROM servers s
         JOIN server_members sm ON sm.server_id = s.id
         WHERE sm.user_id = $1
         ORDER BY s.created_at"
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

pub async fn create_server(pool: &DbPool, name: &str, owner_id: Uuid, invite_code: &str) -> Result<Uuid, sqlx::Error> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO servers (name, owner_id, invite_code) VALUES ($1, $2, $3) RETURNING id"
    )
    .bind(name)
    .bind(owner_id)
    .bind(invite_code)
    .fetch_one(pool)
    .await?;

    let server_id = row.0;

    // Add owner as founder member
    sqlx::query(
        "INSERT INTO server_members (user_id, server_id, role) VALUES ($1, $2, 'founder')"
    )
    .bind(owner_id)
    .bind(server_id)
    .execute(pool)
    .await?;

    // Create default category + channel
    let cat_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO categories (server_id, name, position) VALUES ($1, 'GENERAL', 0) RETURNING id"
    )
    .bind(server_id)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "INSERT INTO channels (category_id, server_id, name, channel_type, topic, position)
         VALUES ($1, $2, 'general', 'text', 'Welcome!', 0)"
    )
    .bind(cat_id.0)
    .bind(server_id)
    .execute(pool)
    .await?;

    Ok(server_id)
}

pub async fn join_server_by_invite(pool: &DbPool, user_id: Uuid, invite_code: &str) -> Result<Option<Uuid>, sqlx::Error> {
    let server = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM servers WHERE invite_code = $1"
    )
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

async fn get_server_categories(pool: &DbPool, server_id: Uuid) -> Result<Vec<CategoryPayload>, sqlx::Error> {
    let cat_rows = sqlx::query_as::<_, CategoryRow>(
        "SELECT id, name FROM categories WHERE server_id = $1 ORDER BY position"
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
            channels: channels.into_iter().map(|ch| ChannelPayload {
                id: ch.id,
                name: ch.name,
                channel_type: parse_channel_type(&ch.channel_type),
                topic: ch.topic,
            }).collect(),
        });
    }
    Ok(categories)
}

async fn get_server_members(pool: &DbPool, server_id: Uuid) -> Result<Vec<MemberPayload>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id, u.username, u.avatar_url, sm.role, u.status, u.custom_status
         FROM users u
         JOIN server_members sm ON sm.user_id = u.id
         WHERE sm.server_id = $1
         ORDER BY sm.role, u.username"
    )
    .bind(server_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| MemberPayload {
        user_id: r.id,
        username: r.username,
        avatar_url: r.avatar_url,
        role: parse_role(&r.role),
        status: parse_status(&r.status),
        custom_status: r.custom_status,
    }).collect())
}

// ── Message queries ──

pub async fn create_message(pool: &DbPool, channel_id: Uuid, author_id: Uuid, content: &str) -> Result<Message, sqlx::Error> {
    let row = sqlx::query_as::<_, MessageRow>(
        "INSERT INTO messages (channel_id, author_id, content)
         VALUES ($1, $2, $3)
         RETURNING id, channel_id, author_id, content, edited_at, created_at"
    )
    .bind(channel_id)
    .bind(author_id)
    .bind(content)
    .fetch_one(pool)
    .await?;

    let author = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT username, avatar_url FROM users WHERE id = $1"
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
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at
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
            "SELECT m.id, m.channel_id, m.author_id, m.content, m.edited_at, m.created_at
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
            "SELECT username, avatar_url FROM users WHERE id = $1"
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
        });
    }

    messages.reverse();
    Ok(messages)
}

pub async fn edit_message(pool: &DbPool, message_id: Uuid, author_id: Uuid, content: &str) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    let result = sqlx::query_as::<_, (DateTime<Utc>,)>(
        "UPDATE messages SET content = $1, edited_at = NOW()
         WHERE id = $2 AND author_id = $3
         RETURNING edited_at"
    )
    .bind(content)
    .bind(message_id)
    .bind(author_id)
    .fetch_optional(pool)
    .await?;

    Ok(result.map(|r| r.0))
}

pub async fn delete_message(pool: &DbPool, message_id: Uuid, author_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM messages WHERE id = $1 AND author_id = $2"
    )
    .bind(message_id)
    .bind(author_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn create_channel(
    pool: &DbPool,
    server_id: Uuid,
    category_id: Option<Uuid>,
    name: &str,
) -> Result<ChannelPayload, sqlx::Error> {
    let cat_id = if let Some(cid) = category_id {
        cid
    } else {
        // Get first category of the server
        let row: (Uuid,) = sqlx::query_as(
            "SELECT id FROM categories WHERE server_id = $1 ORDER BY position LIMIT 1"
        )
        .bind(server_id)
        .fetch_one(pool)
        .await?;
        row.0
    };

    let row = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO channels (category_id, server_id, name, channel_type, position)
         VALUES ($1, $2, $3, 'text', (SELECT COALESCE(MAX(position), 0) + 1 FROM channels WHERE category_id = $1))
         RETURNING id"
    )
    .bind(cat_id)
    .bind(server_id)
    .bind(name)
    .fetch_one(pool)
    .await?;

    Ok(ChannelPayload {
        id: row.0,
        name: name.to_string(),
        channel_type: ChannelType::Text,
        topic: None,
    })
}

// ── Row types ──

#[derive(sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub github_id: Option<i64>,
    pub status: String,
    pub custom_status: Option<String>,
    pub auth_token: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ServerRow {
    id: Uuid,
    name: String,
    icon_url: Option<String>,
    owner_id: Uuid,
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
}

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
