//! The relay served on a loopback port and driven over HTTP and a real
//! WebSocket.
//!
//! The tests marked #[ignore] need a Postgres server: set DATABASE_URL and run
//! `cargo test -p artemis-server -- --include-ignored`. sqlx::test gives each
//! one its own database with the migrations applied. CI runs them in the
//! relay-db job.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use artemis_core::protocol::ClientEvent;

use crate::db::DbPool;
use crate::state::AppState;

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

const FOUNDER: Uuid = Uuid::from_u128(0xa1);
const MEMBER: Uuid = Uuid::from_u128(0xa2);
/// Founder of server B, not a member of server A. One of their messages is
/// still in A, as if they had left it.
const OUTSIDER: Uuid = Uuid::from_u128(0xb1);
/// In no server, with no friends.
const STRANGER: Uuid = Uuid::from_u128(0xd1);

const SERVER_A: Uuid = Uuid::from_u128(0x5a);
const SERVER_B: Uuid = Uuid::from_u128(0x5b);
const CATEGORY_A: Uuid = Uuid::from_u128(0xca);
const CATEGORY_B: Uuid = Uuid::from_u128(0xcb);
const CHANNEL_A: Uuid = Uuid::from_u128(0xc0a);
const CHANNEL_B: Uuid = Uuid::from_u128(0xc0b);

/// Pinned in CHANNEL_A.
const FOUNDER_MESSAGE: Uuid = Uuid::from_u128(0xe1);
const MEMBER_MESSAGE: Uuid = Uuid::from_u128(0xe2);
const OUTSIDER_MESSAGE: Uuid = Uuid::from_u128(0xe3);

/// Users sign in with their name followed by "-token".
async fn seed(pool: &DbPool) {
    let sql = format!(
        "INSERT INTO users (id, username, github_id) VALUES
            ('{FOUNDER}', 'founder', 1),
            ('{MEMBER}', 'member', 2),
            ('{OUTSIDER}', 'outsider', 3),
            ('{STRANGER}', 'stranger', 4);
         INSERT INTO sessions (token_hash, user_id, expires_at)
            SELECT sha256(convert_to(username || '-token', 'UTF8')), id, NOW() + INTERVAL '1 day'
            FROM users;
         INSERT INTO servers (id, name, owner_id, invite_code) VALUES
            ('{SERVER_A}', 'A', '{FOUNDER}', 'invite-a'),
            ('{SERVER_B}', 'B', '{OUTSIDER}', 'invite-b');
         INSERT INTO server_members (user_id, server_id, role) VALUES
            ('{FOUNDER}', '{SERVER_A}', 'founder'),
            ('{MEMBER}', '{SERVER_A}', 'member'),
            ('{OUTSIDER}', '{SERVER_B}', 'founder');
         INSERT INTO categories (id, server_id, name) VALUES
            ('{CATEGORY_A}', '{SERVER_A}', 'GENERAL'),
            ('{CATEGORY_B}', '{SERVER_B}', 'GENERAL');
         INSERT INTO channels (id, category_id, server_id, name) VALUES
            ('{CHANNEL_A}', '{CATEGORY_A}', '{SERVER_A}', 'general'),
            ('{CHANNEL_B}', '{CATEGORY_B}', '{SERVER_B}', 'general');
         INSERT INTO messages (id, channel_id, author_id, content) VALUES
            ('{FOUNDER_MESSAGE}', '{CHANNEL_A}', '{FOUNDER}', 'hello from the founder'),
            ('{MEMBER_MESSAGE}', '{CHANNEL_A}', '{MEMBER}', 'hello from a member'),
            ('{OUTSIDER_MESSAGE}', '{CHANNEL_A}', '{OUTSIDER}', 'hello before leaving');
         INSERT INTO pinned_messages (channel_id, message_id, pinned_by) VALUES
            ('{CHANNEL_A}', '{FOUNDER_MESSAGE}', '{FOUNDER}');"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await
        .unwrap();
}

/// Everything the gated events can change.
async fn snapshot(pool: &DbPool) -> String {
    sqlx::query_scalar(
        "SELECT concat_ws(' | ',
            (SELECT string_agg(id::text || ' ' || content || ' ' || (edited_at IS NULL)::text, ', ' ORDER BY id)
             FROM messages),
            (SELECT string_agg(message_id::text, ', ' ORDER BY message_id) FROM pinned_messages),
            (SELECT COUNT(*) FROM message_reactions),
            (SELECT COUNT(*) FROM channel_read_state),
            (SELECT string_agg(id::text, ', ' ORDER BY id) FROM channels))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn app_state(db: DbPool) -> AppState {
    AppState {
        db,
        connections: super::new_connection_map(),
        presence: Default::default(),
        unauthenticated: Arc::new(Semaphore::new(super::MAX_UNAUTHENTICATED)),
        // Unused: the device flow tests swap in stub_github.
        github: artemis_auth::GitHub::new("test-client").unwrap(),
    }
}

/// A pool that never connects, for tests that do not reach the database. A
/// lookup through it fails within 100 ms, so a sign-in gets AuthError.
fn unused_pool() -> DbPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(100))
        .connect_lazy("postgres://127.0.0.1/unused")
        .unwrap()
}

/// Serves the relay's routes on a free loopback port.
async fn serve(db: DbPool) -> SocketAddr {
    serve_state(app_state(db)).await
}

async fn serve_state(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, crate::router(state)).await });
    addr
}

/// The next event the relay sends. Fails after 10 seconds, so an event the
/// relay drops without an answer fails the test instead of hanging it.
async fn recv(ws: &mut Client) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("no answer within 10 seconds")
            .expect("the relay closed the socket")
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

/// Fails if the relay sends anything within 500 ms.
async fn assert_silent(ws: &mut Client) {
    if let Ok(frame) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
        panic!("expected nothing, got {frame:?}");
    }
}

async fn send(ws: &mut Client, event: &ClientEvent) {
    let json = serde_json::to_string(event).unwrap();
    ws.send(Message::Text(json.into())).await.unwrap();
}

async fn call(ws: &mut Client, event: &ClientEvent) -> Value {
    send(ws, event).await;
    recv(ws).await
}

async fn connect(addr: SocketAddr) -> tokio_tungstenite::tungstenite::Result<Client> {
    Ok(tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await?
        .0)
}

async fn sign_in(addr: SocketAddr, token: &str) -> Client {
    let mut ws = connect(addr).await.unwrap();
    let event = ClientEvent::Authenticate {
        token: token.to_string(),
    };
    assert_eq!(call(&mut ws, &event).await["type"], "Authenticated");
    assert_eq!(recv(&mut ws).await["type"], "UnreadState");
    ws
}

/// Every event that reads or writes inside server A that any member may send,
/// with the answer a member gets, in an order where each one succeeds.
/// `own_message` is a message in A written by the sender.
fn member_events(own_message: Uuid) -> Vec<(ClientEvent, &'static str)> {
    let search = |channel_id| ClientEvent::SearchMessages {
        server_id: SERVER_A,
        channel_id,
        query: "hello".to_string(),
        limit: 10,
    };
    vec![
        (
            ClientEvent::SendMessage {
                channel_id: CHANNEL_A,
                content: "hi".to_string(),
                reply_to_id: None,
                client_nonce: None,
            },
            "MessageReceived",
        ),
        (
            ClientEvent::SendMessage {
                channel_id: CHANNEL_A,
                content: "a reply".to_string(),
                reply_to_id: Some(FOUNDER_MESSAGE),
                client_nonce: None,
            },
            "MessageReceived",
        ),
        (
            ClientEvent::FetchMessages {
                channel_id: CHANNEL_A,
                before: None,
                limit: 50,
            },
            "MessageHistory",
        ),
        (
            ClientEvent::FetchPinnedMessages {
                channel_id: CHANNEL_A,
            },
            "PinnedMessages",
        ),
        (search(None), "SearchResults"),
        (search(Some(CHANNEL_A)), "SearchResults"),
        (
            ClientEvent::AddReaction {
                message_id: FOUNDER_MESSAGE,
                emoji: "+1".to_string(),
            },
            "ReactionAdded",
        ),
        (
            ClientEvent::RemoveReaction {
                message_id: FOUNDER_MESSAGE,
                emoji: "+1".to_string(),
            },
            "ReactionRemoved",
        ),
        (
            ClientEvent::EditMessage {
                message_id: own_message,
                content: "edited".to_string(),
            },
            "MessageEdited",
        ),
        (
            ClientEvent::DeleteMessage {
                message_id: own_message,
            },
            "MessageDeleted",
        ),
        (
            ClientEvent::GetInviteCode {
                server_id: SERVER_A,
            },
            "InviteCode",
        ),
        (
            ClientEvent::FetchCustomEmojis {
                server_id: SERVER_A,
            },
            "CustomEmojiList",
        ),
    ]
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn members_read_and_write_in_their_server(pool: DbPool) {
    seed(&pool).await;
    let mut member = sign_in(serve(pool.clone()).await, "member-token").await;

    for (event, answer) in member_events(MEMBER_MESSAGE) {
        let reply = call(&mut member, &event).await;
        assert_eq!(reply["type"], answer, "{event:?} got {reply}");
    }

    // Typing and read marks get no answer, so the sentinel's answer comes
    // next, and the read mark is stored.
    send(
        &mut member,
        &ClientEvent::StartTyping {
            channel_id: CHANNEL_A,
        },
    )
    .await;
    send(
        &mut member,
        &ClientEvent::AckMessage {
            channel_id: CHANNEL_A,
            message_id: FOUNDER_MESSAGE,
        },
    )
    .await;
    assert_eq!(
        call(&mut member, &ClientEvent::FetchFriends).await["type"],
        "FriendList"
    );
    let read_marks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM channel_read_state WHERE user_id = $1")
            .bind(MEMBER)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(read_marks, 1);
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn non_members_get_an_error_and_nothing_is_read_or_written(pool: DbPool) {
    seed(&pool).await;
    let before = snapshot(&pool).await;
    let mut outsider = sign_in(serve(pool.clone()).await, "outsider-token").await;

    let mut events: Vec<ClientEvent> = member_events(OUTSIDER_MESSAGE)
        .into_iter()
        .map(|(event, _)| event)
        .collect();
    events.extend([
        // A search in the outsider's own server, aimed at a channel of A.
        ClientEvent::SearchMessages {
            server_id: SERVER_B,
            channel_id: Some(CHANNEL_A),
            query: "hello".to_string(),
            limit: 10,
        },
        // A reply in the outsider's own server to a message of A.
        ClientEvent::SendMessage {
            channel_id: CHANNEL_B,
            content: "a reply".to_string(),
            reply_to_id: Some(MEMBER_MESSAGE),
            client_nonce: None,
        },
        ClientEvent::StartTyping {
            channel_id: CHANNEL_A,
        },
        ClientEvent::AckMessage {
            channel_id: CHANNEL_A,
            message_id: FOUNDER_MESSAGE,
        },
        // A read mark in the outsider's own channel for a message of A.
        ClientEvent::AckMessage {
            channel_id: CHANNEL_B,
            message_id: FOUNDER_MESSAGE,
        },
        ClientEvent::PinMessage {
            message_id: MEMBER_MESSAGE,
        },
        ClientEvent::UnpinMessage {
            message_id: FOUNDER_MESSAGE,
        },
        // An id that does not exist gets the same answer.
        ClientEvent::FetchMessages {
            channel_id: Uuid::from_u128(0xdead),
            before: None,
            limit: 50,
        },
    ]);

    for event in &events {
        let reply = call(&mut outsider, event).await;
        assert_eq!(reply["type"], "Error", "{event:?} got {reply}");
    }
    // Each refusal was the only answer: the next event's answer comes next.
    assert_eq!(
        call(&mut outsider, &ClientEvent::FetchFriends).await["type"],
        "FriendList"
    );
    assert_eq!(snapshot(&pool).await, before);
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn only_founders_and_moderators_pin_and_unpin(pool: DbPool) {
    seed(&pool).await;
    let before = snapshot(&pool).await;
    let mut member = sign_in(serve(pool.clone()).await, "member-token").await;
    let pin = ClientEvent::PinMessage {
        message_id: MEMBER_MESSAGE,
    };
    let unpin = ClientEvent::UnpinMessage {
        message_id: FOUNDER_MESSAGE,
    };

    assert_eq!(call(&mut member, &pin).await["type"], "Error");
    assert_eq!(call(&mut member, &unpin).await["type"], "Error");
    assert_eq!(snapshot(&pool).await, before);

    sqlx::query("UPDATE server_members SET role = 'moderator' WHERE user_id = $1")
        .bind(MEMBER)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(call(&mut member, &pin).await["type"], "MessagePinned");
    assert_eq!(call(&mut member, &unpin).await["type"], "MessageUnpinned");
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn channels_are_created_only_in_a_category_of_their_server(pool: DbPool) {
    seed(&pool).await;
    let before = snapshot(&pool).await;
    // The outsider founded B, so they may create channels there but not in A.
    let mut founder_b = sign_in(serve(pool.clone()).await, "outsider-token").await;
    let create = |server_id, category_id| ClientEvent::CreateChannel {
        server_id,
        name: "new".to_string(),
        category_id,
    };

    for event in [
        create(SERVER_B, Some(CATEGORY_A)),
        create(SERVER_A, Some(CATEGORY_A)),
        create(SERVER_B, Some(Uuid::from_u128(0xdead))),
    ] {
        let reply = call(&mut founder_b, &event).await;
        assert_eq!(reply["type"], "Error", "{event:?} got {reply}");
    }
    assert_eq!(snapshot(&pool).await, before);

    // Without a category the channel goes into the server's first one.
    for event in [create(SERVER_B, Some(CATEGORY_B)), create(SERVER_B, None)] {
        let reply = call(&mut founder_b, &event).await;
        assert_eq!(reply["type"], "ChannelCreated", "{event:?} got {reply}");
        assert_eq!(reply["data"]["category_id"], CATEGORY_B.to_string());
    }
    let in_b: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM channels WHERE category_id = $1 AND name = 'new'")
            .bind(CATEGORY_B)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(in_b, 2);
}

/// Signs in a stranger, server B's founder, A's member and A's founder, in
/// that order, and takes the member's notice that the founder came online.
async fn sign_in_everyone(addr: SocketAddr) -> [Client; 4] {
    let stranger = sign_in(addr, "stranger-token").await;
    let outsider = sign_in(addr, "outsider-token").await;
    let mut member = sign_in(addr, "member-token").await;
    let founder = sign_in(addr, "founder-token").await;
    assert_eq!(recv(&mut member).await["type"], "PresenceUpdate");
    [stranger, outsider, member, founder]
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn server_events_reach_only_that_servers_members(pool: DbPool) {
    seed(&pool).await;
    let [mut stranger, mut outsider, mut member, mut founder] =
        sign_in_everyone(serve(pool).await).await;

    let events = [
        (
            ClientEvent::SendMessage {
                channel_id: CHANNEL_A,
                content: "hi".to_string(),
                reply_to_id: None,
                client_nonce: None,
            },
            "MessageReceived",
        ),
        (
            ClientEvent::EditMessage {
                message_id: FOUNDER_MESSAGE,
                content: "edited".to_string(),
            },
            "MessageEdited",
        ),
        (
            ClientEvent::AddReaction {
                message_id: MEMBER_MESSAGE,
                emoji: "+1".to_string(),
            },
            "ReactionAdded",
        ),
        (
            ClientEvent::RemoveReaction {
                message_id: MEMBER_MESSAGE,
                emoji: "+1".to_string(),
            },
            "ReactionRemoved",
        ),
        (
            ClientEvent::PinMessage {
                message_id: MEMBER_MESSAGE,
            },
            "MessagePinned",
        ),
        (
            ClientEvent::UnpinMessage {
                message_id: FOUNDER_MESSAGE,
            },
            "MessageUnpinned",
        ),
        (
            ClientEvent::DeleteMessage {
                message_id: FOUNDER_MESSAGE,
            },
            "MessageDeleted",
        ),
        (
            ClientEvent::CreateCategory {
                server_id: SERVER_A,
                name: "MORE".to_string(),
            },
            "CategoryCreated",
        ),
        (
            ClientEvent::CreateChannel {
                server_id: SERVER_A,
                name: "new".to_string(),
                category_id: None,
            },
            "ChannelCreated",
        ),
        (
            ClientEvent::EditChannel {
                channel_id: CHANNEL_A,
                name: Some("renamed".to_string()),
                topic: None,
            },
            "ChannelUpdated",
        ),
        (
            ClientEvent::EditServer {
                server_id: SERVER_A,
                name: Some("A2".to_string()),
                icon_url: None,
            },
            "ServerUpdated",
        ),
    ];
    for (event, kind) in &events {
        let reply = call(&mut founder, event).await;
        assert_eq!(reply["type"], *kind, "{event:?} got {reply}");
        assert_eq!(recv(&mut member).await["type"], *kind, "{event:?}");
    }

    // An emoji is added, then removed by the id the relay gave it.
    let add = ClientEvent::AddCustomEmoji {
        server_id: SERVER_A,
        name: "wave".to_string(),
        image_url: "https://example.com/wave.png".to_string(),
    };
    let added = call(&mut founder, &add).await;
    assert_eq!(added["type"], "CustomEmojiAdded", "got {added}");
    assert_eq!(recv(&mut member).await, added);
    let remove = ClientEvent::RemoveCustomEmoji {
        server_id: SERVER_A,
        emoji_id: serde_json::from_value(added["data"]["emoji"]["id"].clone()).unwrap(),
    };
    let removed = call(&mut founder, &remove).await;
    assert_eq!(removed["type"], "CustomEmojiRemoved", "got {removed}");
    assert_eq!(recv(&mut member).await, removed);

    // Typing goes to the other members, then the channel goes.
    let typing = ClientEvent::StartTyping {
        channel_id: CHANNEL_A,
    };
    send(&mut founder, &typing).await;
    assert_eq!(recv(&mut member).await["type"], "UserTyping");
    let delete = ClientEvent::DeleteChannel {
        channel_id: CHANNEL_A,
    };
    assert_eq!(call(&mut founder, &delete).await["type"], "ChannelDeleted");
    assert_eq!(recv(&mut member).await["type"], "ChannelDeleted");

    tokio::join!(assert_silent(&mut stranger), assert_silent(&mut outsider));
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn server_deleted_reaches_every_former_member(pool: DbPool) {
    seed(&pool).await;
    let [mut stranger, mut outsider, mut member, mut founder] =
        sign_in_everyone(serve(pool).await).await;

    let delete = ClientEvent::DeleteServer {
        server_id: SERVER_A,
    };
    let deleted = json!({"type": "ServerDeleted", "data": {"server_id": SERVER_A}});
    assert_eq!(call(&mut founder, &delete).await, deleted);
    assert_eq!(recv(&mut member).await, deleted);

    tokio::join!(assert_silent(&mut stranger), assert_silent(&mut outsider));
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn member_left_reaches_the_leaver_and_the_remaining_members(pool: DbPool) {
    seed(&pool).await;
    let [mut stranger, mut outsider, mut member, mut founder] =
        sign_in_everyone(serve(pool).await).await;

    let leave = ClientEvent::LeaveServer {
        server_id: SERVER_A,
    };
    let left = json!({"type": "MemberLeft", "data": {"server_id": SERVER_A, "user_id": MEMBER}});
    assert_eq!(call(&mut member, &leave).await, left);
    assert_eq!(recv(&mut founder).await, left);

    // From then on the leaver hears nothing from the server.
    let post = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: "still here".to_string(),
        reply_to_id: None,
        client_nonce: None,
    };
    assert_eq!(call(&mut founder, &post).await["type"], "MessageReceived");

    tokio::join!(
        assert_silent(&mut stranger),
        assert_silent(&mut outsider),
        assert_silent(&mut member),
    );
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn presence_reaches_co_members_and_friends_only(pool: DbPool) {
    seed(&pool).await;
    // The member shares server A with the founder and is friends with the
    // outsider, who is not in A.
    sqlx::query("INSERT INTO friendships (user_a, user_b) VALUES ($1, $2)")
        .bind(MEMBER)
        .bind(OUTSIDER)
        .execute(&pool)
        .await
        .unwrap();
    let addr = serve(pool).await;
    let mut stranger = sign_in(addr, "stranger-token").await;
    let mut founder = sign_in(addr, "founder-token").await;
    let mut outsider = sign_in(addr, "outsider-token").await;
    let mut member = sign_in(addr, "member-token").await;

    let about_member = |kind: &str, status: &str| json!({"type": kind, "data": {"user_id": MEMBER, "status": status}});
    assert_eq!(
        recv(&mut founder).await,
        about_member("PresenceUpdate", "Online")
    );
    assert_eq!(
        recv(&mut outsider).await,
        about_member("FriendPresenceUpdate", "Online")
    );

    let idle = ClientEvent::UpdatePresence {
        status: artemis_core::models::user::UserStatus::Idle,
    };
    send(&mut member, &idle).await;
    assert_eq!(
        recv(&mut founder).await,
        about_member("PresenceUpdate", "Idle")
    );
    // The member's own devices hear their own change.
    assert_eq!(
        recv(&mut member).await,
        about_member("PresenceUpdate", "Idle")
    );
    assert_eq!(
        recv(&mut outsider).await,
        about_member("FriendPresenceUpdate", "Idle")
    );

    // A profile change goes to co-members and the member's own devices only.
    let profile = ClientEvent::UpdateProfile {
        display_name: Some("Bob".to_string()),
        custom_status: None,
    };
    send(&mut member, &profile).await;
    let updated = recv(&mut founder).await;
    assert_eq!(updated["type"], "ProfileUpdated");
    assert_eq!(updated["data"]["user_id"], MEMBER.to_string());
    assert_eq!(recv(&mut member).await, updated);

    member.close(None).await.unwrap();
    assert_eq!(
        recv(&mut founder).await,
        about_member("PresenceUpdate", "Offline")
    );
    assert_eq!(
        recv(&mut outsider).await,
        about_member("FriendPresenceUpdate", "Offline")
    );

    tokio::join!(
        assert_silent(&mut stranger),
        assert_silent(&mut outsider),
        assert_silent(&mut founder),
    );
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_new_public_key_reaches_friends_only(pool: DbPool) {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;

    seed(&pool).await;
    // The member is friends with the outsider, and shares server A with the
    // founder without being friends.
    sqlx::query("INSERT INTO friendships (user_a, user_b) VALUES ($1, $2)")
        .bind(MEMBER)
        .bind(OUTSIDER)
        .execute(&pool)
        .await
        .unwrap();
    let addr = serve(pool).await;
    let mut founder = sign_in(addr, "founder-token").await;
    let mut outsider = sign_in(addr, "outsider-token").await;
    let mut member = sign_in(addr, "member-token").await;
    assert_eq!(recv(&mut founder).await["type"], "PresenceUpdate");
    assert_eq!(recv(&mut outsider).await["type"], "FriendPresenceUpdate");

    let publish = |key: &str| ClientEvent::PublishPublicKey {
        public_key: key.to_string(),
    };
    let changed = |key: &str| json!({"type": "FriendKeyChanged", "data": {"user_id": MEMBER, "public_key": key}});
    let first = B64.encode([1u8; 32]);
    let second = B64.encode([2u8; 32]);

    for key in [&first, &second] {
        assert_eq!(
            call(&mut member, &publish(key)).await["type"],
            "PublicKeyAcknowledged"
        );
        assert_eq!(recv(&mut outsider).await, changed(key));
    }
    // The same key again is no change.
    assert_eq!(
        call(&mut member, &publish(&second)).await["type"],
        "PublicKeyAcknowledged"
    );
    tokio::join!(
        assert_silent(&mut founder),
        assert_silent(&mut outsider),
        assert_silent(&mut member),
    );
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_declined_request_can_be_sent_again_and_friends_can_part(pool: DbPool) {
    seed(&pool).await;
    let addr = serve(pool).await;
    // They share no server, so neither hears the other come online.
    let mut founder = sign_in(addr, "founder-token").await;
    let mut stranger = sign_in(addr, "stranger-token").await;
    let request = ClientEvent::SendFriendRequest {
        target_username: "founder".to_string(),
    };
    let decline = ClientEvent::DeclineFriendRequest {
        from_user_id: STRANGER,
    };

    send(&mut stranger, &request).await;
    assert_eq!(recv(&mut founder).await["type"], "FriendRequestReceived");
    // Asking again while the request is pending notifies nobody.
    send(&mut stranger, &request).await;
    assert_silent(&mut founder).await;

    send(&mut founder, &decline).await;
    assert_eq!(
        recv(&mut stranger).await,
        json!({"type": "FriendRequestDeclined", "data": {"by_user_id": FOUNDER}})
    );
    // Nothing is pending any more, so a second decline answers nobody.
    send(&mut founder, &decline).await;
    assert_silent(&mut stranger).await;

    // Sent again after the decline, the request can be accepted.
    send(&mut stranger, &request).await;
    assert_eq!(recv(&mut founder).await["type"], "FriendRequestReceived");
    let accept = ClientEvent::AcceptFriendRequest {
        from_user_id: STRANGER,
    };
    send(&mut founder, &accept).await;
    assert_eq!(recv(&mut stranger).await["type"], "FriendRequestAccepted");
    assert_eq!(recv(&mut founder).await["type"], "FriendRequestAccepted");

    let remove = ClientEvent::RemoveFriend { user_id: STRANGER };
    send(&mut founder, &remove).await;
    let removed = |user_id| json!({"type": "FriendRemoved", "data": {"user_id": user_id}});
    assert_eq!(recv(&mut stranger).await, removed(FOUNDER));
    assert_eq!(recv(&mut founder).await, removed(STRANGER));
    let dm = ClientEvent::SendDirectMessage {
        recipient_id: FOUNDER,
        encrypted_content: "sealed".to_string(),
        client_nonce: None,
    };
    assert_eq!(
        call(&mut stranger, &dm).await,
        json!({"type": "Error", "data": {"message": "Not friends with this user"}})
    );
    assert_eq!(
        call(&mut founder, &remove).await,
        json!({"type": "Error", "data": {"message": "Not friends with this user"}})
    );

    // Either side may ask again.
    send(&mut stranger, &request).await;
    assert_eq!(recv(&mut founder).await["type"], "FriendRequestReceived");
}

async fn member_status(pool: &DbPool) -> String {
    sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(MEMBER)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn member_presence(status: &str) -> Value {
    json!({"type": "PresenceUpdate", "data": {"user_id": MEMBER, "status": status}})
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_user_goes_offline_when_their_last_socket_closes(pool: DbPool) {
    seed(&pool).await;
    let addr = serve(pool.clone()).await;
    let mut founder = sign_in(addr, "founder-token").await;
    let mut phone = sign_in(addr, "member-token").await;
    assert_eq!(recv(&mut founder).await, member_presence("Online"));

    // The member steps away on the phone, then opens the desktop. The second
    // socket announces nothing and keeps the status the member chose.
    let idle = ClientEvent::UpdatePresence {
        status: artemis_core::models::user::UserStatus::Idle,
    };
    send(&mut phone, &idle).await;
    assert_eq!(recv(&mut founder).await, member_presence("Idle"));
    assert_eq!(recv(&mut phone).await, member_presence("Idle"));
    let mut desktop = sign_in(addr, "member-token").await;
    assert_silent(&mut founder).await;
    assert_eq!(member_status(&pool).await, "idle");

    phone.close(None).await.unwrap();
    assert_silent(&mut founder).await;
    assert_eq!(member_status(&pool).await, "idle");

    // The other socket still gets the member's events.
    let post = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: "still there?".to_string(),
        reply_to_id: None,
        client_nonce: None,
    };
    assert_eq!(call(&mut founder, &post).await["type"], "MessageReceived");
    assert_eq!(recv(&mut desktop).await["type"], "MessageReceived");

    desktop.close(None).await.unwrap();
    assert_eq!(recv(&mut founder).await, member_presence("Offline"));
    assert_eq!(member_status(&pool).await, "offline");
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_sign_in_racing_the_last_close_leaves_the_user_online(pool: DbPool) {
    seed(&pool).await;
    let state = app_state(pool.clone());
    let addr = serve_state(state.clone()).await;
    let mut founder = sign_in(addr, "founder-token").await;
    let mut old = sign_in(addr, "member-token").await;
    assert_eq!(recv(&mut founder).await, member_presence("Online"));

    // The member's only socket closes as a new one signs in. Both wait for
    // the presence lock, so the close cannot go offline in the meantime.
    let lock = state.presence.lock().await;
    old.close(None).await.unwrap();
    let new = tokio::spawn(sign_in(addr, "member-token"));
    assert_silent(&mut founder).await;
    drop(lock);
    let _new = new.await.unwrap();
    // Both have had their turn once the lock is free again.
    drop(state.presence.lock().await);

    // Whichever went first, the member is stored online, and others heard
    // either nothing or Offline then Online.
    assert_eq!(member_status(&pool).await, "online");
    let mut heard = Vec::new();
    while let Ok(frame) = tokio::time::timeout(Duration::from_millis(500), recv(&mut founder)).await
    {
        heard.push(frame);
    }
    assert!(
        heard.is_empty() || heard == [member_presence("Offline"), member_presence("Online")],
        "{heard:?}"
    );
}

/// Frames up to and including the next MessageReceived, each of which has to
/// arrive within 2 seconds.
async fn until_message(ws: &mut Client) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), recv(ws))
            .await
            .expect("nothing within 2 seconds");
        let done = frame["type"] == "MessageReceived";
        frames.push(frame);
        if done {
            return frames;
        }
    }
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_client_that_stops_reading_holds_up_nobody(pool: DbPool) {
    seed(&pool).await;
    sqlx::query("INSERT INTO server_members (user_id, server_id, role) VALUES ($1, $2, 'member')")
        .bind(STRANGER)
        .bind(SERVER_A)
        .execute(&pool)
        .await
        .unwrap();
    let addr = serve(pool).await;

    // The member signs in, then never reads again.
    let _stuck = sign_in(addr, "member-token").await;
    let mut watcher = sign_in(addr, "stranger-token").await;
    // A socket may send EVENTS_PER_SECOND events, so the founder posts from
    // as many sockets as one user may open, in turn, each at that rate.
    let mut posters = Vec::new();
    for _ in 0..super::MAX_SOCKETS_PER_USER {
        posters.push(sign_in(addr, "founder-token").await);
    }
    let mut pace = tokio::time::interval(Duration::from_secs_f64(
        1.0 / (super::EVENTS_PER_SECOND * posters.len() as f64),
    ));
    pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    assert_eq!(recv(&mut watcher).await["type"], "PresenceUpdate");

    // Messages as large as allowed fill the stuck socket's buffers, then its
    // queue. Once the queue is full, or a frame has waited SEND_TIMEOUT, the
    // relay drops that socket, and the member goes offline. Meanwhile
    // everyone else keeps getting each message at once.
    let post = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: "x".repeat(artemis_core::limits::MAX_MESSAGE_CHARS),
        reply_to_id: None,
        client_nonce: None,
    };
    let offline =
        json!({"type": "PresenceUpdate", "data": {"user_id": MEMBER, "status": "Offline"}});
    let mut dropped = false;
    for sent in 1..=8000 {
        pace.tick().await;
        send(&mut posters[sent % super::MAX_SOCKETS_PER_USER], &post).await;
        for poster in &mut posters {
            until_message(poster).await;
        }
        dropped |= until_message(&mut watcher).await.contains(&offline);
        if sent % 64 == 0 {
            // Signing in is not held up either. The outsider shares no server
            // with anyone here and has no friends, so nobody hears of it.
            tokio::time::timeout(Duration::from_secs(2), sign_in(addr, "outsider-token"))
                .await
                .expect("no sign-in within 2 seconds");
        }
        if dropped {
            break;
        }
    }
    assert!(dropped, "the stuck socket was never dropped");
}

#[tokio::test]
async fn a_full_queue_drops_only_its_socket() {
    use super::{ConnectedUser, Outbox};

    let state = app_state(unused_pool());
    let users = [1, 2, 3].map(Uuid::from_u128);
    let mut queues = Vec::new();
    let mut writers = Vec::new();
    for (conn, user_id) in users.into_iter().enumerate() {
        let (queue, rx) = Outbox::new();
        let writer = tokio::spawn(std::future::pending::<()>());
        let entry = ConnectedUser {
            user_id,
            queue,
            writer: writer.abort_handle(),
        };
        state
            .connections
            .write()
            .await
            .insert(Uuid::from_u128(conn as u128), entry);
        queues.push(rx);
        writers.push(writer);
    }
    // The third socket has not taken a frame as large as its whole queue.
    let third = state.connections.read().await[&Uuid::from_u128(2)]
        .queue
        .clone();
    assert!(third.try_send("x".repeat(super::SEND_QUEUE_BYTES).into()));

    let event = artemis_core::protocol::ServerEvent::PublicKeyAcknowledged;
    super::send_to_users(&state, &users.into(), &event).await;

    let frame = serde_json::to_string(&event).unwrap();
    for rx in &mut queues[..2] {
        assert_eq!(rx.try_recv().unwrap().0.as_str(), frame);
    }
    let connections = state.connections.read().await;
    let mut left: Vec<_> = connections.keys().map(|id| id.as_u128()).collect();
    left.sort();
    assert_eq!(left, [0, 1]);
    drop(connections);

    let third_writer = writers.pop().unwrap();
    assert!(third_writer.await.unwrap_err().is_cancelled());
    assert!(writers.iter().all(|writer| !writer.is_finished()));
}

#[tokio::test]
async fn a_send_queue_holds_at_most_its_byte_budget() {
    let (outbox, mut queue) = super::Outbox::new();
    let half = || "x".repeat(super::SEND_QUEUE_BYTES / 2).into();
    assert!(outbox.try_send(half()));
    assert!(outbox.try_send(half()));
    // Two frames, far from SEND_QUEUE, but every byte is taken. The fan-out
    // gives up and the socket's own reader waits.
    assert!(!outbox.try_send("x".into()));
    let wait = tokio::time::timeout(Duration::from_millis(100), outbox.send("x".into()));
    assert!(wait.await.is_err(), "a reply was queued past the budget");

    // A frame gives its room back as the writer takes it.
    drop(queue.recv().await.unwrap());
    outbox.send("x".into()).await.unwrap();

    // A frame larger than the whole budget goes in once the queue is empty.
    let too_large = || "x".repeat(super::SEND_QUEUE_BYTES * 3).into();
    assert!(!outbox.try_send(too_large()));
    drop(queue.recv().await.unwrap());
    drop(queue.recv().await.unwrap());
    assert!(outbox.try_send(too_large()));
}

#[tokio::test(start_paused = true)]
async fn a_socket_that_misses_a_ping_is_dropped() {
    // The writer on its own, since a socket that has not signed in is closed
    // long before the first ping. The test reads what it sends.
    let (sink, mut sent) = futures::channel::mpsc::unbounded();
    let (_queue, frames) = super::Outbox::new();
    let ponged = Arc::new(AtomicBool::new(true));
    let writer = tokio::spawn(super::write_frames(sink, frames, ponged.clone()));
    let is_ping = |frame| matches!(frame, Some(axum::extract::ws::Message::Ping(_)));

    // A client that answers every ping, as the reader records each pong,
    // stays connected through four of them.
    for _ in 0..4 {
        assert!(is_ping(sent.next().await));
        ponged.store(true, Ordering::Relaxed);
    }
    // Once it stops answering, the next ping finds the last one unanswered.
    assert!(is_ping(sent.next().await));
    let missed = tokio::time::Instant::now();
    writer.await.unwrap();
    assert_eq!(missed.elapsed(), super::PING_INTERVAL);
    assert_eq!(sent.next().await, None);
}

/// Pings `ws` gets until `until`. Fails on any other frame, or if the relay
/// closes the socket.
async fn pings_until(ws: &mut Client, until: tokio::time::Instant) -> usize {
    let mut pings = 0;
    while let Ok(frame) = tokio::time::timeout_at(until, ws.next()).await {
        match frame {
            Some(Ok(Message::Ping(_))) => pings += 1,
            other => panic!("expected a ping, got {other:?}"),
        }
    }
    pings
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_client_that_answers_pings_stays_connected(pool: DbPool) {
    seed(&pool).await;
    let mut ws = sign_in(serve(pool).await, "stranger-token").await;

    // Signed in on the real clock, since a paused one jumps ahead while the
    // database answers. From here only pings and pongs go back and forth.
    // The paused clock jumps to the next timer whenever the runtime idles,
    // even while a frame is on its way. A 10 ms ticker keeps the jumps short,
    // so a pong reaches the relay long before the next ping.
    tokio::time::pause();
    let ticker = tokio::spawn(async {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tick.tick().await;
        }
    });
    let start = tokio::time::Instant::now();

    // The client answers each ping as it reads. Pings go out at 30, 60 and
    // 90 s; had the relay not recorded the pongs, it would close at 60 s.
    let until = start + super::PING_INTERVAL * 3 + Duration::from_secs(5);
    assert_eq!(pings_until(&mut ws, until).await, 3);

    // sqlx drops the test database after this, on the real clock.
    ticker.abort();
    tokio::time::resume();
}

#[tokio::test(start_paused = true)]
async fn a_frame_that_cannot_go_out_in_time_ends_the_writer() {
    // A sink that never takes the frame, like a client whose buffers are full.
    let (sink, _never_read) = futures::channel::mpsc::channel::<axum::extract::ws::Message>(0);
    let (queue, frames) = super::Outbox::new();
    let writer = tokio::spawn(super::write_frames(sink, frames, Default::default()));
    queue.send("stuck".into()).await.unwrap();
    let start = tokio::time::Instant::now();

    tokio::time::timeout(super::SEND_TIMEOUT * 2, writer)
        .await
        .expect("the writer still waits on the frame")
        .unwrap();
    assert!(start.elapsed() >= super::SEND_TIMEOUT);
}

#[tokio::test]
async fn rest_routes_that_took_the_token_in_the_query_are_gone() {
    // Never connects: none of these routes reaches the database.
    let addr = serve(unused_pool()).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    let removed = [
        http.get(format!("http://{addr}/api/v1/servers?token=t")),
        http.post(format!("http://{addr}/api/v1/servers?token=t")),
        http.get(format!("http://{addr}/api/v1/me?token=t")),
        http.get(format!(
            "http://{addr}/api/v1/channels/{CHANNEL_A}/messages?token=t"
        )),
    ];
    for request in removed {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 404, "{}", response.url());
    }

    let health = http.get(format!("http://{addr}/health")).send().await;
    assert_eq!(health.unwrap().status(), 200);
}

/// The relay closed `ws`: it ended, failed, or got a close frame.
fn closed(frame: &Option<tokio_tungstenite::tungstenite::Result<Message>>) -> bool {
    matches!(frame, None | Some(Err(_)) | Some(Ok(Message::Close(_))))
}

#[tokio::test]
async fn a_frame_over_64_kib_closes_the_socket() {
    let mut ws = connect(serve(unused_pool()).await).await.unwrap();
    // A well-formed Authenticate, so only its size can close the socket.
    // Read in full, it would get an AuthError.
    let event = ClientEvent::Authenticate {
        token: "t".repeat(128 * 1024),
    };
    let json = serde_json::to_string(&event).unwrap();
    // The relay may close the socket before the frame is fully written.
    let _ = ws.send(Message::Text(json.into())).await;

    let end = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("the socket is still open");
    assert!(closed(&end), "{end:?}");
}

#[test]
fn a_message_at_the_character_cap_fits_in_a_frame() {
    use artemis_core::limits::MAX_MESSAGE_CHARS;
    // Control characters are escaped as six bytes each, the most any
    // character takes in JSON.
    let event = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: "\u{1}".repeat(MAX_MESSAGE_CHARS),
        reply_to_id: Some(FOUNDER_MESSAGE),
        client_nonce: Some(Uuid::max()),
    };
    let frame = serde_json::to_string(&event).unwrap();
    assert!(frame.len() < super::MAX_MESSAGE, "{} bytes", frame.len());
}

#[test]
fn a_direct_message_at_the_ciphertext_cap_fits_in_a_frame() {
    use artemis_core::limits::MAX_DM_CIPHERTEXT_BYTES;
    let event = ClientEvent::SendDirectMessage {
        recipient_id: MEMBER,
        encrypted_content: "A".repeat(MAX_DM_CIPHERTEXT_BYTES),
        client_nonce: Some(Uuid::max()),
    };
    let frame = serde_json::to_string(&event).unwrap();
    assert!(frame.len() < super::MAX_MESSAGE, "{} bytes", frame.len());
}

#[test]
fn mentions_are_counted_once_each_up_to_the_cap() {
    use artemis_core::limits::MAX_MENTIONS;
    assert_eq!(super::mentions(&"@lin ".repeat(500)), ["lin"]);
    assert_eq!(super::mentions("@lin, @lin! @ @. mail@lin"), ["lin"]);
    let many: String = (0..30).map(|i| format!("@user{i} ")).collect();
    let mut found = super::mentions(&many);
    found.sort();
    let mut first: Vec<String> = (0..MAX_MENTIONS).map(|i| format!("user{i}")).collect();
    first.sort();
    assert_eq!(found, first);
}

#[tokio::test(start_paused = true)]
async fn a_socket_that_does_not_sign_in_is_closed_after_the_deadline() {
    // The paused clock jumps to the next timer whenever the runtime idles,
    // even while a frame is on its way. A 10 ms ticker keeps the jumps short.
    tokio::spawn(async {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tick.tick().await;
        }
    });
    let addr = serve(unused_pool()).await;
    let start = tokio::time::Instant::now();
    let mut ws = connect(addr).await.unwrap();

    // Closed, not pinged: the first ping would come at 30 seconds.
    let end = ws.next().await;
    assert!(closed(&end), "{end:?}");
    assert!(start.elapsed() >= super::AUTH_DEADLINE);
}

#[tokio::test]
async fn anything_before_authenticate_closes_the_socket() {
    let addr = serve(unused_pool()).await;
    let fetch = serde_json::to_string(&ClientEvent::FetchFriends).unwrap();
    for first in [fetch, "not json".to_string()] {
        let mut ws = connect(addr).await.unwrap();
        ws.send(Message::Text(first.into())).await.unwrap();
        // Well before AUTH_DEADLINE, and with no reply.
        let end = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("the socket is still open");
        assert!(closed(&end), "{end:?}");
    }
}

#[tokio::test]
async fn a_refusal_reaches_a_client_that_sent_more_after_authenticate() {
    let mut ws = connect(serve(unused_pool()).await).await.unwrap();
    // As the shipped client does: Authenticate, then more events at once,
    // which reach the relay while it looks up the token.
    let events = [
        ClientEvent::Authenticate {
            token: "t".to_string(),
        },
        ClientEvent::FetchFriends,
        ClientEvent::FetchFriendRequests,
    ];
    for event in &events {
        let json = serde_json::to_string(event).unwrap();
        ws.feed(Message::Text(json.into())).await.unwrap();
    }
    ws.flush().await.unwrap();
    assert_eq!(recv(&mut ws).await["type"], "AuthError");

    // A socket gets one try: another Authenticate is not even looked up.
    send(&mut ws, &events[0]).await;
    assert_silent(&mut ws).await;
}

#[tokio::test]
async fn sockets_over_the_unauthenticated_cap_are_refused() {
    let state = AppState {
        unauthenticated: Arc::new(Semaphore::new(1)),
        ..app_state(unused_pool())
    };
    let addr = serve_state(state).await;
    let first = connect(addr).await.unwrap();

    let refused = connect(addr).await.err().unwrap();
    assert!(
        matches!(&refused, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 503),
        "{refused:?}"
    );

    // Closing the first socket gives its place back.
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), async {
        while connect(addr).await.is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the closed socket kept its place");
}

#[tokio::test(start_paused = true)]
async fn the_event_bucket_allows_a_burst_then_its_rate() {
    let mut bucket = super::EventBucket::new();
    let burst = (0..1000).filter(|_| bucket.take()).count();
    assert_eq!(burst, super::EVENT_BURST as usize);

    // A client that keeps to the rate is never cut off.
    for _ in 0..10 {
        tokio::time::advance(Duration::from_secs(1)).await;
        let second = (0..1000).filter(|_| bucket.take()).count();
        assert_eq!(second, super::EVENTS_PER_SECOND as usize);
    }
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn an_invalid_event_is_answered_without_quoting_it(pool: DbPool) {
    seed(&pool).await;
    let mut member = sign_in(serve(pool).await, "member-token").await;

    // A string where serde expects a struct, which its error quotes in full,
    // and text that is not JSON at all. Both stay under the size cap.
    let quoted = format!("quote-me-{}", "x".repeat(60 * 1024));
    let frames = [
        json!({"type": "SendMessage", "data": quoted}).to_string(),
        format!("not json {quoted}"),
    ];
    for frame in frames {
        member.send(Message::Text(frame.into())).await.unwrap();
        assert_eq!(
            recv(&mut member).await,
            json!({"type": "Error", "data": {"message": "Invalid event"}})
        );
    }
    // The socket stays open.
    assert_eq!(
        call(&mut member, &ClientEvent::FetchFriends).await["type"],
        "FriendList"
    );
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_second_authenticate_changes_nothing(pool: DbPool) {
    seed(&pool).await;
    let mut member = sign_in(serve(pool.clone()).await, "member-token").await;

    let again = ClientEvent::Authenticate {
        token: "founder-token".to_string(),
    };
    assert_eq!(
        call(&mut member, &again).await,
        json!({"type": "Error", "data": {"message": "Already authenticated"}})
    );

    // The socket still speaks for the member, and the founder never came
    // online.
    let post = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: "who am I".to_string(),
        reply_to_id: None,
        client_nonce: None,
    };
    let posted = call(&mut member, &post).await;
    assert_eq!(posted["data"]["message"]["author_id"], MEMBER.to_string());
    let founder: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(FOUNDER)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(founder, "offline");
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn two_hundred_events_in_a_second_close_the_socket(pool: DbPool) {
    seed(&pool).await;
    let mut stranger = sign_in(serve(pool).await, "stranger-token").await;

    let fetch = serde_json::to_string(&ClientEvent::FetchFriends).unwrap();
    for _ in 0..200 {
        // The relay may close the socket before the last ones are written.
        let _ = stranger.feed(Message::Text(fetch.clone().into())).await;
    }
    let _ = stranger.flush().await;

    let mut answered = 0;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), stranger.next())
            .await
            .expect("the socket is still open");
        if closed(&frame) {
            break;
        }
        answered += 1;
    }
    assert!(answered < 200, "{answered} events answered");
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_sixth_socket_for_one_user_is_refused(pool: DbPool) {
    seed(&pool).await;
    // Room for one socket that has not signed in, so each sign-in below has
    // to give its place back for the next socket to connect.
    let state = AppState {
        unauthenticated: Arc::new(Semaphore::new(1)),
        ..app_state(pool)
    };
    let addr = serve_state(state).await;
    let mut open = Vec::new();
    for _ in 0..super::MAX_SOCKETS_PER_USER {
        open.push(sign_in(addr, "member-token").await);
    }

    let mut sixth = connect(addr).await.unwrap();
    let event = ClientEvent::Authenticate {
        token: "member-token".to_string(),
    };
    assert_eq!(
        call(&mut sixth, &event).await,
        json!({"type": "AuthError", "data": {"reason": "Too many connections for this account"}})
    );
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn messages_over_the_character_cap_are_refused(pool: DbPool) {
    use artemis_core::limits::MAX_MESSAGE_CHARS;
    seed(&pool).await;
    let mut member = sign_in(serve(pool.clone()).await, "member-token").await;
    let before = snapshot(&pool).await;

    // Counted in characters: each of these is two bytes.
    let over = "\u{e9}".repeat(MAX_MESSAGE_CHARS + 1);
    let post = |content: String| ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content,
        reply_to_id: None,
        client_nonce: None,
    };
    let edit = ClientEvent::EditMessage {
        message_id: MEMBER_MESSAGE,
        content: over.clone(),
    };
    let refused = json!({"type": "Error", "data": {"message": format!(
        "A message may have at most {MAX_MESSAGE_CHARS} characters"
    )}});
    assert_eq!(call(&mut member, &post(over)).await, refused);
    assert_eq!(call(&mut member, &edit).await, refused);
    assert_eq!(snapshot(&pool).await, before);

    let at_cap = post("\u{e9}".repeat(MAX_MESSAGE_CHARS));
    assert_eq!(call(&mut member, &at_cap).await["type"], "MessageReceived");
}

#[sqlx::test]
#[ignore = "needs DATABASE_URL"]
async fn the_sender_gets_its_client_nonce_back(pool: DbPool) {
    seed(&pool).await;
    sqlx::query("INSERT INTO friendships (user_a, user_b) VALUES ($1, $2)")
        .bind(FOUNDER)
        .bind(MEMBER)
        .execute(&pool)
        .await
        .unwrap();
    let addr = serve(pool).await;
    let mut member = sign_in(addr, "member-token").await;
    let mut founder = sign_in(addr, "founder-token").await;
    // The member hears that the founder, a co-member and a friend, came online.
    assert_eq!(recv(&mut member).await["type"], "PresenceUpdate");
    assert_eq!(recv(&mut member).await["type"], "FriendPresenceUpdate");
    let nonce = Uuid::from_u128(0x40);

    let post = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: "hi".to_string(),
        reply_to_id: None,
        client_nonce: Some(nonce),
    };
    let echo = call(&mut founder, &post).await;
    assert_eq!(echo["type"], "MessageReceived");
    assert_eq!(echo["data"]["client_nonce"], nonce.to_string());
    assert_eq!(recv(&mut member).await, echo);

    let dm = ClientEvent::SendDirectMessage {
        recipient_id: MEMBER,
        encrypted_content: "sealed".to_string(),
        client_nonce: Some(nonce),
    };
    let echo = call(&mut founder, &dm).await;
    assert_eq!(echo["type"], "DirectMessageReceived");
    assert_eq!(echo["data"]["client_nonce"], nonce.to_string());
    // The recipient gets the same message without the sender's nonce.
    let received = recv(&mut member).await;
    assert_eq!(received["data"]["message_id"], echo["data"]["message_id"]);
    assert!(received["data"].get("client_nonce").is_none(), "{received}");
}

/// Server A's name and icon, its channels, categories and emojis, and every
/// profile: what the refused events below would have changed.
async fn names(pool: &DbPool) -> String {
    sqlx::query_scalar(
        "SELECT concat_ws(' | ',
            (SELECT name || ' ' || COALESCE(icon_url, '-') FROM servers WHERE id = $1),
            (SELECT string_agg(name || ' ' || COALESCE(topic, '-'), ', ' ORDER BY id) FROM channels),
            (SELECT string_agg(name, ', ' ORDER BY id) FROM categories),
            (SELECT COUNT(*) FROM custom_emojis),
            (SELECT string_agg(COALESCE(display_name, '-') || ' ' || COALESCE(custom_status, '-'), ', ' ORDER BY id)
             FROM users))",
    )
    .bind(SERVER_A)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn texts_over_their_limits_are_refused_and_never_broadcast(pool: DbPool) {
    use artemis_core::limits::{MAX_DM_CIPHERTEXT_BYTES, MAX_NAME_CHARS};
    seed(&pool).await;
    sqlx::query("UPDATE server_members SET role = 'moderator' WHERE user_id = $1")
        .bind(MEMBER)
        .execute(&pool)
        .await
        .unwrap();
    let addr = serve(pool.clone()).await;
    let mut founder = sign_in(addr, "founder-token").await;
    let mut moderator = sign_in(addr, "member-token").await;
    assert_eq!(recv(&mut founder).await, member_presence("Online"));
    let before = (snapshot(&pool).await, names(&pool).await);

    let long_name = "n".repeat(70);
    let text = |s: &str| Some(s.to_string());
    let refused = [
        ClientEvent::EditServer {
            server_id: SERVER_A,
            name: Some(long_name.clone()),
            icon_url: None,
        },
        // A valid name with a bad icon changes neither.
        ClientEvent::EditServer {
            server_id: SERVER_A,
            name: text("A2"),
            icon_url: text("https://example.com/\0.png"),
        },
        ClientEvent::UpdateProfile {
            display_name: None,
            custom_status: Some("s".repeat(129)),
        },
        ClientEvent::UpdateProfile {
            display_name: Some("d".repeat(MAX_NAME_CHARS + 1)),
            custom_status: text("fine"),
        },
        ClientEvent::EditChannel {
            channel_id: CHANNEL_A,
            name: text("renamed"),
            topic: Some("t".repeat(1025)),
        },
        ClientEvent::EditChannel {
            channel_id: CHANNEL_A,
            name: text("a\0b"),
            topic: None,
        },
        ClientEvent::CreateChannel {
            server_id: SERVER_A,
            name: long_name.clone(),
            category_id: None,
        },
        ClientEvent::CreateCategory {
            server_id: SERVER_A,
            name: long_name.clone(),
        },
        ClientEvent::AddCustomEmoji {
            server_id: SERVER_A,
            name: long_name.clone(),
            image_url: "https://example.com/wave.png".to_string(),
        },
        ClientEvent::AddReaction {
            message_id: FOUNDER_MESSAGE,
            emoji: long_name.clone(),
        },
        ClientEvent::SendMessage {
            channel_id: CHANNEL_A,
            content: "a\0b".to_string(),
            reply_to_id: None,
            client_nonce: None,
        },
        ClientEvent::SendDirectMessage {
            recipient_id: FOUNDER,
            encrypted_content: "A".repeat(MAX_DM_CIPHERTEXT_BYTES + 1),
            client_nonce: None,
        },
    ];
    // Each refusal names the limit, not a permission or a database failure.
    for event in &refused {
        let reply = call(&mut moderator, event).await;
        let message = reply["data"]["message"].as_str().unwrap_or_default();
        assert!(
            reply["type"] == "Error" && (message.contains("at most") || message.contains("NUL")),
            "{event:?} got {reply}"
        );
    }
    assert_eq!(
        call(
            &mut moderator,
            &ClientEvent::EditServer {
                server_id: SERVER_A,
                name: Some(long_name),
                icon_url: None,
            }
        )
        .await,
        json!({"type": "Error", "data": {"message": "A server name may have at most 64 characters"}})
    );
    assert_silent(&mut founder).await;
    assert_eq!((snapshot(&pool).await, names(&pool).await), before);

    // An accepted edit goes out as stored, with only the fields it set, so
    // edits of different fields apply in any order.
    let icon = ClientEvent::EditServer {
        server_id: SERVER_A,
        name: None,
        icon_url: text("https://example.com/a.png"),
    };
    let updated = json!({"type": "ServerUpdated", "data": {
        "server_id": SERVER_A, "name": null, "icon_url": "https://example.com/a.png",
    }});
    assert_eq!(call(&mut moderator, &icon).await, updated);
    assert_eq!(recv(&mut founder).await, updated);
    let rename = ClientEvent::EditServer {
        server_id: SERVER_A,
        name: text("A2"),
        icon_url: None,
    };
    let updated = json!({"type": "ServerUpdated", "data": {
        "server_id": SERVER_A, "name": "A2", "icon_url": null,
    }});
    assert_eq!(call(&mut moderator, &rename).await, updated);
    assert_eq!(recv(&mut founder).await, updated);
    assert!(names(&pool)
        .await
        .starts_with("A2 https://example.com/a.png |"));
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn only_a_32_byte_public_key_is_stored(pool: DbPool) {
    seed(&pool).await;
    let mut member = sign_in(serve(pool.clone()).await, "member-token").await;
    let publish = |key: String| ClientEvent::PublishPublicKey { public_key: key };
    let refused = json!({"type": "Error", "data": {
        "message": "A public key must be 32 bytes in base64",
    }});
    // Junk, a NUL, 31 and 33 bytes, and unpadded base64.
    for key in [
        "x".repeat(60_000),
        "\0".to_string(),
        "A".repeat(40) + "AA==",
        "A".repeat(44),
        "A".repeat(43),
    ] {
        assert_eq!(call(&mut member, &publish(key)).await, refused);
    }
    let stored = || async {
        let key: Option<String> = sqlx::query_scalar("SELECT public_key FROM users WHERE id = $1")
            .bind(MEMBER)
            .fetch_one(&pool)
            .await
            .unwrap();
        key
    };
    assert_eq!(stored().await, None);

    let key = "A".repeat(43) + "=";
    let reply = call(&mut member, &publish(key.clone())).await;
    assert_eq!(reply["type"], "PublicKeyAcknowledged");
    assert_eq!(stored().await, Some(key));
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_name_mentioned_many_times_counts_once(pool: DbPool) {
    seed(&pool).await;
    const LIN: Uuid = Uuid::from_u128(0x11);
    sqlx::query("INSERT INTO users (id, username, github_id) VALUES ($1, 'lin', 5)")
        .bind(LIN)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO server_members (user_id, server_id) VALUES ($1, $2)")
        .bind(LIN)
        .bind(SERVER_A)
        .execute(&pool)
        .await
        .unwrap();
    let mut founder = sign_in(serve(pool.clone()).await, "founder-token").await;

    // The founder mentions themselves too, which does not count.
    let post = ClientEvent::SendMessage {
        channel_id: CHANNEL_A,
        content: format!("@founder {}", "@lin ".repeat(500)),
        reply_to_id: None,
        client_nonce: None,
    };
    assert_eq!(call(&mut founder, &post).await["type"], "MessageReceived");

    let counts: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT user_id, mention_count FROM channel_read_state WHERE channel_id = $1",
    )
    .bind(CHANNEL_A)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(counts, [(LIN, 1)]);
}

/// A stand-in for GitHub on a loopback port, returning its base URL. The
/// device code picks GitHub's answer: `pending`, `slow_down`, `denied`,
/// `expired` and `unknown` get those errors, and `<id>:<login>` grants a
/// token for GitHub user <id> with that login.
async fn stub_github() -> String {
    use std::collections::HashMap;

    use axum::extract::Form;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use axum::Json;

    type FormData = Form<HashMap<String, String>>;
    let app = axum::Router::new()
        .route(
            "/login/device/code",
            post(|Form(form): FormData| async move {
                assert_eq!(form["client_id"], "test-client");
                assert_eq!(form["scope"], "read:user");
                Json(json!({
                    "device_code": "dc",
                    "user_code": "ABCD-1234",
                    "verification_uri": "https://github.com/login/device",
                    "expires_in": 900,
                    "interval": 5,
                }))
            }),
        )
        .route(
            "/login/oauth/access_token",
            post(|Form(form): FormData| async move {
                assert_eq!(form["client_id"], "test-client");
                Json(match form["device_code"].as_str() {
                    "pending" => json!({"error": "authorization_pending"}),
                    "slow_down" => json!({"error": "slow_down", "interval": 10}),
                    "denied" => json!({"error": "access_denied"}),
                    "expired" => json!({"error": "expired_token"}),
                    "unknown" => json!({"error": "incorrect_device_code"}),
                    user => json!({"access_token": format!("gho_{user}"), "token_type": "bearer"}),
                })
            }),
        )
        .route(
            "/user",
            get(|headers: HeaderMap| async move {
                let token = headers["authorization"].to_str().unwrap();
                let user = token.strip_prefix("Bearer gho_").unwrap();
                let (id, login) = user.split_once(':').unwrap();
                Json(json!({
                    "id": id.parse::<i64>().unwrap(),
                    "login": login,
                    "name": "Ada Lovelace",
                    "avatar_url": format!("https://avatars.example/{id}"),
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

/// The relay, signing in through stub_github.
async fn serve_device_flow(db: DbPool) -> SocketAddr {
    let github = stub_github().await;
    serve_state(AppState {
        github: artemis_auth::GitHub::with_urls("test-client", &github, &github).unwrap(),
        ..app_state(db)
    })
    .await
}

/// POSTs to a device flow route and returns the status and the JSON body.
async fn post_json(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

async fn poll(addr: SocketAddr, device_code: &str) -> (u16, Value) {
    let body = json!({ "device_code": device_code });
    post_json(addr, "/auth/device/poll", body).await
}

#[tokio::test]
async fn the_device_flow_passes_on_githubs_answers() {
    // Never connects: only a granted token reaches the database.
    let addr = serve_device_flow(unused_pool()).await;

    assert_eq!(
        post_json(addr, "/auth/device/start", json!({})).await,
        (
            200,
            json!({
                "device_code": "dc",
                "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 5,
            })
        )
    );
    let too_long = "7".repeat(65);
    let answers = [
        ("pending", 200, json!({"status": "pending"})),
        (
            "slow_down",
            200,
            json!({"status": "slow_down", "interval": 10}),
        ),
        ("denied", 200, json!({"status": "denied"})),
        ("expired", 200, json!({"status": "expired"})),
        ("unknown", 502, json!({"error": "GitHub sign-in failed"})),
        // Never sent to GitHub, whose stub would answer with a token.
        ("", 400, json!({"error": "Invalid device code"})),
        ("1:a b", 400, json!({"error": "Invalid device code"})),
        (
            too_long.as_str(),
            400,
            json!({"error": "Invalid device code"}),
        ),
        // GitHub granted a token, then the database failed: no session.
        ("7:ada", 500, json!({"error": "Database error"})),
    ];
    for (device_code, status, body) in answers {
        assert_eq!(
            poll(addr, device_code).await,
            (status, body),
            "{device_code}"
        );
    }
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_device_sign_in_opens_a_hashed_session(pool: DbPool) {
    let addr = serve_device_flow(pool.clone()).await;

    let (status, body) = poll(addr, "7:ada").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "complete");
    assert_eq!(body["username"], "ada");
    assert_eq!(body["avatar_url"], "https://avatars.example/7");
    let token = body["token"].as_str().unwrap();

    let (hash, sha256, days): (Vec<u8>, Vec<u8>, f64) = sqlx::query_as(
        "SELECT token_hash, sha256(convert_to($1, 'UTF8')),
                EXTRACT(EPOCH FROM expires_at - NOW())::float8 / 86400
         FROM sessions",
    )
    .bind(token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(hash, token.as_bytes());
    assert_eq!(hash, sha256);
    assert!((29.9..=30.0).contains(&days), "{days}");

    let mut ws = connect(addr).await.unwrap();
    let event = ClientEvent::Authenticate {
        token: token.to_string(),
    };
    let signed_in = call(&mut ws, &event).await;
    assert_eq!(signed_in["type"], "Authenticated");
    assert_eq!(signed_in["data"]["username"], "ada");
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn a_failed_sign_in_stores_nothing(pool: DbPool) {
    let addr = serve_device_flow(pool.clone()).await;
    // The session insert fails, after the account was written.
    sqlx::query("ALTER TABLE sessions ADD CHECK (false)")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        poll(addr, "7:ada").await,
        (500, json!({"error": "Database error"}))
    );
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 0);
}

#[sqlx::test(migrator = "crate::db::MIGRATOR")]
#[ignore = "needs DATABASE_URL"]
async fn an_expired_session_is_refused_and_a_used_one_extended(pool: DbPool) {
    seed(&pool).await;
    sqlx::query("UPDATE sessions SET expires_at = NOW() - INTERVAL '1 second' WHERE user_id = $1")
        .bind(MEMBER)
        .execute(&pool)
        .await
        .unwrap();
    let addr = serve(pool.clone()).await;

    let mut ws = connect(addr).await.unwrap();
    let event = ClientEvent::Authenticate {
        token: "member-token".to_string(),
    };
    assert_eq!(
        call(&mut ws, &event).await,
        json!({"type": "AuthError", "data": {"reason": "Invalid or expired session"}})
    );

    // The seed gave the founder one day left; signing in makes it 30 again.
    let _founder = sign_in(addr, "founder-token").await;
    let days: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM expires_at - NOW())::float8 / 86400 FROM sessions WHERE user_id = $1",
    )
    .bind(FOUNDER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((29.9..=30.0).contains(&days), "{days}");
}
