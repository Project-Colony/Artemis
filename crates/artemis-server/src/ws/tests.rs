//! The relay served on a loopback port and driven over a real WebSocket.
//!
//! The tests marked #[ignore] need a Postgres server: set DATABASE_URL and run
//! `cargo test -p artemis-server -- --include-ignored`. sqlx::test gives each
//! one its own database with the migrations applied. CI runs them in the
//! relay-db job.

use std::net::SocketAddr;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
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
        "INSERT INTO users (id, username, github_id, auth_token) VALUES
            ('{FOUNDER}', 'founder', 1, 'founder-token'),
            ('{MEMBER}', 'member', 2, 'member-token'),
            ('{OUTSIDER}', 'outsider', 3, 'outsider-token'),
            ('{STRANGER}', 'stranger', 4, 'stranger-token');
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

/// Serves the relay's routes on a free loopback port.
async fn serve(db: DbPool) -> SocketAddr {
    let state = AppState {
        db,
        connections: super::new_connection_map(),
        github_client_id: String::new(),
        github_client_secret: String::new(),
        base_url: "http://127.0.0.1".to_string(),
    };
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

async fn sign_in(addr: SocketAddr, token: &str) -> Client {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
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
            },
            "MessageReceived",
        ),
        (
            ClientEvent::SendMessage {
                channel_id: CHANNEL_A,
                content: "a reply".to_string(),
                reply_to_id: Some(FOUNDER_MESSAGE),
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
async fn server_events_reach_its_members_only(pool: DbPool) {
    seed(&pool).await;
    let [mut stranger, mut outsider, mut member, mut founder] =
        sign_in_everyone(serve(pool).await).await;

    let events = [
        (
            ClientEvent::SendMessage {
                channel_id: CHANNEL_A,
                content: "hi".to_string(),
                reply_to_id: None,
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
        (
            ClientEvent::AddCustomEmoji {
                server_id: SERVER_A,
                name: "wave".to_string(),
                image_url: "https://example.com/wave.png".to_string(),
            },
            "CustomEmojiAdded",
        ),
    ];
    for (event, kind) in &events {
        let reply = call(&mut founder, event).await;
        assert_eq!(reply["type"], *kind, "{event:?} got {reply}");
        assert_eq!(recv(&mut member).await["type"], *kind, "{event:?}");
    }

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
    assert_eq!(
        recv(&mut outsider).await,
        about_member("FriendPresenceUpdate", "Idle")
    );

    // A profile change goes to co-members only.
    let profile = ClientEvent::UpdateProfile {
        display_name: Some("Bob".to_string()),
        custom_status: None,
    };
    send(&mut member, &profile).await;
    let updated = recv(&mut founder).await;
    assert_eq!(updated["type"], "ProfileUpdated");
    assert_eq!(updated["data"]["user_id"], MEMBER.to_string());

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

#[tokio::test]
async fn rest_routes_that_took_the_token_in_the_query_are_gone() {
    // Never connects: none of these routes reaches the database.
    let pool = sqlx::PgPool::connect_lazy("postgres://127.0.0.1/unused").unwrap();
    let addr = serve(pool).await;
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
