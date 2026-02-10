use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use artemis_core::protocol::{ClientEvent, ServerEvent, FriendPayload, FriendRequestPayload};

use crate::db;
use crate::state::AppState;

pub fn ws_routes() -> Router<AppState> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));

    let mut user_id: Option<Uuid> = None;
    let conn_id = Uuid::new_v4();

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            WsMessage::Text(txt) => {
                let event: ClientEvent = match serde_json::from_str(&txt) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = send_event(&sender, &ServerEvent::Error {
                            message: format!("Invalid event: {}", e),
                        }).await;
                        continue;
                    }
                };

                match event {
                    ClientEvent::Authenticate { token } => {
                        match db::find_user_by_token(&state.db, &token).await {
                            Ok(Some(user)) => {
                                user_id = Some(user.id);

                                // Register connection
                                state.connections.write().await.insert(conn_id, ConnectedUser {
                                    user_id: user.id,
                                    sender: sender.clone(),
                                });

                                // Set user online
                                let _ = db::update_user_status(&state.db, user.id, "online").await;

                                // Fetch user's servers
                                let servers = db::get_user_servers(&state.db, user.id).await.unwrap_or_default();

                                // Broadcast presence to all connected users
                                broadcast_event(&state, &ServerEvent::PresenceUpdate {
                                    user_id: user.id,
                                    status: artemis_core::models::user::UserStatus::Online,
                                }, Some(user.id)).await;

                                let _ = send_event(&sender, &ServerEvent::Authenticated {
                                    user_id: user.id,
                                    username: user.username.clone(),
                                    servers,
                                }).await;

                                // Notify friends that we're online
                                if let Ok(friends) = db::get_friends(&state.db, user.id).await {
                                    for friend in &friends {
                                        send_to_user(&state, friend.user_id, &ServerEvent::FriendPresenceUpdate {
                                            user_id: user.id,
                                            status: artemis_core::models::user::UserStatus::Online,
                                        }).await;
                                    }
                                }

                                tracing::info!("User {} authenticated", user.id);
                            }
                            Ok(None) => {
                                let _ = send_event(&sender, &ServerEvent::AuthError {
                                    reason: "Invalid token".to_string(),
                                }).await;
                            }
                            Err(e) => {
                                tracing::error!("DB error during auth: {}", e);
                                let _ = send_event(&sender, &ServerEvent::AuthError {
                                    reason: "Internal error".to_string(),
                                }).await;
                            }
                        }
                    }

                    ClientEvent::SendMessage { channel_id, content } => {
                        let Some(uid) = user_id else {
                            let _ = send_event(&sender, &ServerEvent::Error {
                                message: "Not authenticated".to_string(),
                            }).await;
                            continue;
                        };

                        match db::create_message(&state.db, channel_id, uid, &content).await {
                            Ok(message) => {
                                broadcast_event(&state, &ServerEvent::MessageReceived {
                                    message,
                                }, None).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to create message: {}", e);
                                let _ = send_event(&sender, &ServerEvent::Error {
                                    message: "Failed to send message".to_string(),
                                }).await;
                            }
                        }
                    }

                    ClientEvent::EditMessage { message_id, content } => {
                        let Some(uid) = user_id else { continue };

                        if let Ok(Some(edited_at)) = db::edit_message(&state.db, message_id, uid, &content).await {
                            broadcast_event(&state, &ServerEvent::MessageEdited {
                                message_id,
                                content,
                                edited_at,
                            }, None).await;
                        }
                    }

                    ClientEvent::DeleteMessage { message_id } => {
                        let Some(uid) = user_id else { continue };

                        if let Ok(true) = db::delete_message(&state.db, message_id, uid).await {
                            broadcast_event(&state, &ServerEvent::MessageDeleted {
                                message_id,
                            }, None).await;
                        }
                    }

                    ClientEvent::FetchMessages { channel_id, before, limit } => {
                        let Some(_uid) = user_id else { continue };

                        let limit = limit.min(100);
                        match db::get_messages(&state.db, channel_id, before, limit).await {
                            Ok(messages) => {
                                let has_more = messages.len() == limit as usize;
                                let _ = send_event(&sender, &ServerEvent::MessageHistory {
                                    channel_id,
                                    messages,
                                    has_more,
                                }).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to fetch messages: {}", e);
                            }
                        }
                    }

                    ClientEvent::StartTyping { channel_id } => {
                        if let Some(uid) = user_id {
                            let username = {
                                let conns = state.connections.read().await;
                                conns.get(&conn_id).map(|_| "User".to_string())
                            };
                            if let Some(username) = username {
                                broadcast_event(&state, &ServerEvent::UserTyping {
                                    channel_id,
                                    user_id: uid,
                                    username,
                                }, Some(uid)).await;
                            }
                        }
                    }

                    ClientEvent::UpdatePresence { status } => {
                        if let Some(uid) = user_id {
                            let status_str = match status {
                                artemis_core::models::user::UserStatus::Online => "online",
                                artemis_core::models::user::UserStatus::Idle => "idle",
                                artemis_core::models::user::UserStatus::DoNotDisturb => "dnd",
                                artemis_core::models::user::UserStatus::Offline => "offline",
                            };
                            let _ = db::update_user_status(&state.db, uid, status_str).await;
                            broadcast_event(&state, &ServerEvent::PresenceUpdate {
                                user_id: uid,
                                status,
                            }, None).await;
                        }
                    }

                    ClientEvent::JoinServer { invite_code } => {
                        let Some(uid) = user_id else { continue };

                        if let Ok(Some(_server_id)) = db::join_server_by_invite(&state.db, uid, &invite_code).await {
                            // Refresh servers list
                            let servers = db::get_user_servers(&state.db, uid).await.unwrap_or_default();
                            let _ = send_event(&sender, &ServerEvent::Authenticated {
                                user_id: uid,
                                username: String::new(),
                                servers,
                            }).await;
                        }
                    }

                    ClientEvent::CreateChannel { server_id, name, category_id } => {
                        let Some(_uid) = user_id else { continue };

                        match db::create_channel(&state.db, server_id, category_id, &name).await {
                            Ok(_channel) => {
                                // Client will refresh channel list
                            }
                            Err(e) => {
                                tracing::error!("Failed to create channel: {}", e);
                            }
                        }
                    }

                    // ── Friend / DM / E2E relay ──

                    ClientEvent::PublishPublicKey { public_key } => {
                        let Some(uid) = user_id else { continue };
                        if let Err(e) = db::save_public_key(&state.db, uid, &public_key).await {
                            tracing::error!("Failed to save public key: {}", e);
                        } else {
                            let _ = send_event(&sender, &ServerEvent::PublicKeyAcknowledged).await;
                            tracing::info!("Public key stored for user {}", uid);
                        }
                    }

                    ClientEvent::SendFriendRequest { target_username } => {
                        let Some(uid) = user_id else { continue };

                        let target = match db::find_user_by_username(&state.db, &target_username).await {
                            Ok(Some(u)) => u,
                            Ok(None) => {
                                let _ = send_event(&sender, &ServerEvent::Error {
                                    message: format!("User '{}' not found", target_username),
                                }).await;
                                continue;
                            }
                            Err(e) => {
                                tracing::error!("DB error looking up user: {}", e);
                                continue;
                            }
                        };

                        if target.id == uid {
                            let _ = send_event(&sender, &ServerEvent::Error {
                                message: "Cannot add yourself".to_string(),
                            }).await;
                            continue;
                        }

                        // Check if already friends
                        if db::are_friends(&state.db, uid, target.id).await.unwrap_or(false) {
                            let _ = send_event(&sender, &ServerEvent::Error {
                                message: "Already friends".to_string(),
                            }).await;
                            continue;
                        }

                        if let Err(e) = db::send_friend_request(&state.db, uid, target.id).await {
                            tracing::error!("Failed to send friend request: {}", e);
                            continue;
                        }

                        // Look up sender info to notify target
                        let sender_user = db::find_user_by_token(&state.db, "").await; // need username
                        let sender_username = {
                            // Find username from connections or DB
                            let mut name = String::new();
                            if let Ok(Some(u)) = db::find_user_by_github_id(&state.db, 0).await {
                                name = u.username;
                            }
                            // Actually we need the sender's info properly
                            name
                        };
                        let _ = sender_user; // suppress warning

                        // Get sender info from DB
                        if let Ok(Some(from_user)) = sqlx::query_as::<_, db::UserRow>(
                            "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE id = $1"
                        ).bind(uid).fetch_optional(&state.db).await {
                            // Notify recipient if online
                            let _ = sender_username;
                            send_to_user(&state, target.id, &ServerEvent::FriendRequestReceived {
                                from_user_id: uid,
                                from_username: from_user.username,
                                from_avatar_url: from_user.avatar_url,
                            }).await;
                        }

                        tracing::info!("Friend request sent from {} to {}", uid, target.id);
                    }

                    ClientEvent::AcceptFriendRequest { from_user_id } => {
                        let Some(uid) = user_id else { continue };

                        match db::accept_friend_request(&state.db, from_user_id, uid).await {
                            Ok(true) => {
                                // Notify the requester that we accepted
                                if let Ok(Some(accepter)) = sqlx::query_as::<_, db::UserRow>(
                                    "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE id = $1"
                                ).bind(uid).fetch_optional(&state.db).await {
                                    send_to_user(&state, from_user_id, &ServerEvent::FriendRequestAccepted {
                                        user_id: uid,
                                        username: accepter.username.clone(),
                                        avatar_url: accepter.avatar_url.clone(),
                                        public_key: accepter.public_key.clone(),
                                    }).await;

                                    // Also send the requester's info back to the accepter
                                    if let Ok(Some(requester)) = sqlx::query_as::<_, db::UserRow>(
                                        "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE id = $1"
                                    ).bind(from_user_id).fetch_optional(&state.db).await {
                                        let _ = send_event(&sender, &ServerEvent::FriendRequestAccepted {
                                            user_id: from_user_id,
                                            username: requester.username,
                                            avatar_url: requester.avatar_url,
                                            public_key: requester.public_key,
                                        }).await;
                                    }
                                }
                                tracing::info!("Friend request accepted: {} <-> {}", from_user_id, uid);
                            }
                            Ok(false) => {
                                let _ = send_event(&sender, &ServerEvent::Error {
                                    message: "No pending request from this user".to_string(),
                                }).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to accept friend request: {}", e);
                            }
                        }
                    }

                    ClientEvent::DeclineFriendRequest { from_user_id } => {
                        let Some(uid) = user_id else { continue };
                        let _ = db::decline_friend_request(&state.db, from_user_id, uid).await;
                        send_to_user(&state, from_user_id, &ServerEvent::FriendRequestDeclined {
                            by_user_id: uid,
                        }).await;
                    }

                    ClientEvent::FetchFriends => {
                        let Some(uid) = user_id else { continue };
                        match db::get_friends(&state.db, uid).await {
                            Ok(rows) => {
                                let friends: Vec<FriendPayload> = rows.into_iter().map(|r| FriendPayload {
                                    user_id: r.user_id,
                                    username: r.username,
                                    display_name: r.display_name,
                                    avatar_url: r.avatar_url,
                                    public_key: r.public_key,
                                    status: parse_status(&r.status),
                                }).collect();
                                let _ = send_event(&sender, &ServerEvent::FriendList { friends }).await;
                            }
                            Err(e) => tracing::error!("Failed to fetch friends: {}", e),
                        }
                    }

                    ClientEvent::FetchFriendRequests => {
                        let Some(uid) = user_id else { continue };
                        match db::get_pending_friend_requests(&state.db, uid).await {
                            Ok(rows) => {
                                let requests: Vec<FriendRequestPayload> = rows.into_iter().map(|r| FriendRequestPayload {
                                    from_user_id: r.from_user_id,
                                    from_username: r.from_username,
                                    from_avatar_url: r.from_avatar_url,
                                    created_at: r.created_at,
                                }).collect();
                                let _ = send_event(&sender, &ServerEvent::PendingFriendRequests { requests }).await;
                            }
                            Err(e) => tracing::error!("Failed to fetch friend requests: {}", e),
                        }
                    }

                    ClientEvent::SendDirectMessage { recipient_id, encrypted_content } => {
                        let Some(uid) = user_id else { continue };

                        // Verify friendship
                        if !db::are_friends(&state.db, uid, recipient_id).await.unwrap_or(false) {
                            let _ = send_event(&sender, &ServerEvent::Error {
                                message: "Not friends with this user".to_string(),
                            }).await;
                            continue;
                        }

                        // Store and relay
                        match db::store_direct_message(&state.db, uid, recipient_id, &encrypted_content).await {
                            Ok((msg_id, timestamp)) => {
                                // Get sender username
                                let sender_name = sqlx::query_as::<_, (String,)>(
                                    "SELECT username FROM users WHERE id = $1"
                                ).bind(uid).fetch_one(&state.db).await
                                    .map(|r| r.0)
                                    .unwrap_or_else(|_| "Unknown".to_string());

                                // Relay to recipient if online
                                send_to_user(&state, recipient_id, &ServerEvent::DirectMessageReceived {
                                    from_user_id: uid,
                                    from_username: sender_name.clone(),
                                    encrypted_content: encrypted_content.clone(),
                                    timestamp,
                                    message_id: msg_id,
                                }).await;

                                // Echo back to sender for confirmation
                                let _ = send_event(&sender, &ServerEvent::DirectMessageReceived {
                                    from_user_id: uid,
                                    from_username: sender_name,
                                    encrypted_content,
                                    timestamp,
                                    message_id: msg_id,
                                }).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to store DM: {}", e);
                                let _ = send_event(&sender, &ServerEvent::Error {
                                    message: "Failed to send message".to_string(),
                                }).await;
                            }
                        }
                    }
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup on disconnect
    state.connections.write().await.remove(&conn_id);
    if let Some(uid) = user_id {
        let _ = db::update_user_status(&state.db, uid, "offline").await;
        broadcast_event(&state, &ServerEvent::PresenceUpdate {
            user_id: uid,
            status: artemis_core::models::user::UserStatus::Offline,
        }, None).await;

        // Notify friends we went offline
        if let Ok(friends) = db::get_friends(&state.db, uid).await {
            for friend in &friends {
                send_to_user(&state, friend.user_id, &ServerEvent::FriendPresenceUpdate {
                    user_id: uid,
                    status: artemis_core::models::user::UserStatus::Offline,
                }).await;
            }
        }

        tracing::info!("User {} disconnected", uid);
    }
}

pub struct ConnectedUser {
    pub user_id: Uuid,
    pub sender: Arc<Mutex<SplitSink<WebSocket, WsMessage>>>,
}

pub type ConnectionMap = Arc<RwLock<HashMap<Uuid, ConnectedUser>>>;

pub fn new_connection_map() -> ConnectionMap {
    Arc::new(RwLock::new(HashMap::new()))
}

async fn send_event(
    sender: &Arc<Mutex<SplitSink<WebSocket, WsMessage>>>,
    event: &ServerEvent,
) -> Result<(), Box<dyn std::error::Error + Send>> {
    let json = serde_json::to_string(event).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
    sender.lock().await.send(WsMessage::Text(json.into())).await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
    Ok(())
}

/// Send an event to a specific user (if they're online).
async fn send_to_user(state: &AppState, target_user_id: Uuid, event: &ServerEvent) {
    let json = match serde_json::to_string(event) {
        Ok(j) => j,
        Err(_) => return,
    };
    let connections = state.connections.read().await;
    for conn in connections.values() {
        if conn.user_id == target_user_id {
            let _ = conn.sender.lock().await.send(WsMessage::Text(json.clone().into())).await;
        }
    }
}

fn parse_status(s: &str) -> artemis_core::models::user::UserStatus {
    match s {
        "online" => artemis_core::models::user::UserStatus::Online,
        "idle" => artemis_core::models::user::UserStatus::Idle,
        "dnd" => artemis_core::models::user::UserStatus::DoNotDisturb,
        _ => artemis_core::models::user::UserStatus::Offline,
    }
}

async fn broadcast_event(state: &AppState, event: &ServerEvent, exclude_user: Option<Uuid>) {
    let json = match serde_json::to_string(event) {
        Ok(j) => j,
        Err(_) => return,
    };

    let connections = state.connections.read().await;
    for conn in connections.values() {
        if exclude_user.map(|id| id == conn.user_id).unwrap_or(false) {
            continue;
        }
        let _ = conn.sender.lock().await.send(WsMessage::Text(json.clone().into())).await;
    }
}
