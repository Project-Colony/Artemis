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

use artemis_core::protocol::{ClientEvent, ServerEvent};

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
                                    username: user.username,
                                    servers,
                                }).await;

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
