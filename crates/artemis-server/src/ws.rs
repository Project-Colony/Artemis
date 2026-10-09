use std::collections::{HashMap, HashSet};
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

use artemis_core::protocol::{ClientEvent, FriendPayload, FriendRequestPayload, ServerEvent};

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
    let mut username_cache: Option<String> = None;
    let conn_id = Uuid::new_v4();

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            WsMessage::Text(txt) => {
                let event: ClientEvent = match serde_json::from_str(&txt) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = send_event(
                            &sender,
                            &ServerEvent::Error {
                                message: format!("Invalid event: {}", e),
                            },
                        )
                        .await;
                        continue;
                    }
                };

                match event {
                    ClientEvent::Authenticate { token } => {
                        match db::find_user_by_token(&state.db, &token).await {
                            Ok(Some(user)) => {
                                user_id = Some(user.id);
                                username_cache = Some(user.username.clone());

                                // Register connection
                                state.connections.write().await.insert(
                                    conn_id,
                                    ConnectedUser {
                                        user_id: user.id,
                                        sender: sender.clone(),
                                    },
                                );

                                // Set user online
                                let _ = db::update_user_status(&state.db, user.id, "online").await;

                                // Fetch user's servers
                                let servers = db::get_user_servers(&state.db, user.id)
                                    .await
                                    .unwrap_or_default();

                                announce_presence(
                                    &state,
                                    user.id,
                                    artemis_core::models::user::UserStatus::Online,
                                )
                                .await;

                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Authenticated {
                                        user_id: user.id,
                                        username: user.username.clone(),
                                        servers,
                                    },
                                )
                                .await;

                                // Send unread state
                                if let Ok(channels) = db::get_unread_state(&state.db, user.id).await
                                {
                                    let _ =
                                        send_event(&sender, &ServerEvent::UnreadState { channels })
                                            .await;
                                }

                                tracing::info!("User {} authenticated", user.id);
                            }
                            Ok(None) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::AuthError {
                                        reason: "Invalid token".to_string(),
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("DB error during auth: {}", e);
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::AuthError {
                                        reason: "Internal error".to_string(),
                                    },
                                )
                                .await;
                            }
                        }
                    }

                    ClientEvent::SendMessage {
                        channel_id,
                        content,
                        reply_to_id,
                    } => {
                        let Some(uid) = user_id else {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Error {
                                    message: "Not authenticated".to_string(),
                                },
                            )
                            .await;
                            continue;
                        };

                        let server = found(db::channel_server(&state.db, channel_id).await);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Member).await
                        else {
                            continue;
                        };

                        // Parse @mentions before sending
                        let mentioned_usernames: Vec<String> = content
                            .split_whitespace()
                            .filter(|w| w.starts_with('@') && w.len() > 1)
                            .map(|w| {
                                w[1..]
                                    .trim_end_matches(|c: char| {
                                        !c.is_alphanumeric() && c != '-' && c != '_'
                                    })
                                    .to_string()
                            })
                            .collect();

                        match db::create_message(&state.db, channel_id, uid, &content, reply_to_id)
                            .await
                        {
                            Ok(None) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: "The message you reply to is not in this channel"
                                            .to_string(),
                                    },
                                )
                                .await;
                            }
                            Ok(Some(message)) => {
                                // Track @mentions for unread state
                                if !mentioned_usernames.is_empty() {
                                    if let Ok(resolved) = db::resolve_mentions(
                                        &state.db,
                                        server_id,
                                        &mentioned_usernames,
                                    )
                                    .await
                                    {
                                        for (_uname, mentioned_uid) in resolved {
                                            if mentioned_uid != uid {
                                                let _ = db::increment_mention_count(
                                                    &state.db,
                                                    mentioned_uid,
                                                    channel_id,
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                }

                                broadcast_to_server(
                                    &state,
                                    server_id,
                                    &ServerEvent::MessageReceived { message },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to create message: {}", e);
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: "Failed to send message".to_string(),
                                    },
                                )
                                .await;
                            }
                        }
                    }

                    ClientEvent::EditMessage {
                        message_id,
                        content,
                    } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::message_channel(&state.db, message_id).await)
                            .map(|(_, server)| server);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Member).await
                        else {
                            continue;
                        };

                        if let Ok(Some(edited_at)) =
                            db::edit_message(&state.db, message_id, uid, &content).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::MessageEdited {
                                    message_id,
                                    content,
                                    edited_at,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::DeleteMessage { message_id } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::message_channel(&state.db, message_id).await)
                            .map(|(_, server)| server);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Member).await
                        else {
                            continue;
                        };

                        if let Ok(true) = db::delete_message(&state.db, message_id, uid).await {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::MessageDeleted { message_id },
                            )
                            .await;
                        }
                    }

                    ClientEvent::FetchMessages {
                        channel_id,
                        before,
                        limit,
                    } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::channel_server(&state.db, channel_id).await);
                        if authorize(&state, &sender, uid, server, Need::Member)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        let limit = limit.min(100);
                        match db::get_messages(&state.db, channel_id, before, limit).await {
                            Ok(messages) => {
                                let has_more = messages.len() == limit as usize;
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::MessageHistory {
                                        channel_id,
                                        messages,
                                        has_more,
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to fetch messages: {}", e);
                            }
                        }
                    }

                    ClientEvent::StartTyping { channel_id } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::channel_server(&state.db, channel_id).await);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Member).await
                        else {
                            continue;
                        };

                        let uname = username_cache.clone().unwrap_or_else(|| "User".to_string());
                        let mut others = listed(db::member_ids(&state.db, server_id).await);
                        others.remove(&uid);
                        send_to_users(
                            &state,
                            &others,
                            &ServerEvent::UserTyping {
                                channel_id,
                                user_id: uid,
                                username: uname,
                            },
                        )
                        .await;
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
                            announce_presence(&state, uid, status).await;
                        }
                    }

                    ClientEvent::JoinServer { invite_code } => {
                        let Some(uid) = user_id else { continue };

                        if let Ok(Some(_server_id)) =
                            db::join_server_by_invite(&state.db, uid, &invite_code).await
                        {
                            // Refresh servers list
                            let servers = db::get_user_servers(&state.db, uid)
                                .await
                                .unwrap_or_default();
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Authenticated {
                                    user_id: uid,
                                    username: username_cache.clone().unwrap_or_default(),
                                    servers,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::CreateChannel {
                        server_id,
                        name,
                        category_id,
                    } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Moderator)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        match db::create_channel(&state.db, server_id, category_id, &name).await {
                            Ok(Some((category_id, channel))) => {
                                broadcast_to_server(
                                    &state,
                                    server_id,
                                    &ServerEvent::ChannelCreated {
                                        server_id,
                                        category_id,
                                        channel,
                                    },
                                )
                                .await;
                            }
                            Ok(None) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: "No such category in this server".to_string(),
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to create channel: {}", e);
                            }
                        }
                    }

                    // ── Reactions ──
                    ClientEvent::AddReaction { message_id, emoji } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::message_channel(&state.db, message_id).await)
                            .map(|(_, server)| server);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Member).await
                        else {
                            continue;
                        };

                        if let Ok(true) = db::add_reaction(&state.db, message_id, uid, &emoji).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::ReactionAdded {
                                    message_id,
                                    user_id: uid,
                                    emoji,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::RemoveReaction { message_id, emoji } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::message_channel(&state.db, message_id).await)
                            .map(|(_, server)| server);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Member).await
                        else {
                            continue;
                        };

                        if let Ok(true) =
                            db::remove_reaction(&state.db, message_id, uid, &emoji).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::ReactionRemoved {
                                    message_id,
                                    user_id: uid,
                                    emoji,
                                },
                            )
                            .await;
                        }
                    }

                    // ── Pins ──
                    ClientEvent::PinMessage { message_id } => {
                        let Some(uid) = user_id else { continue };

                        let place = found(db::message_channel(&state.db, message_id).await);
                        let server = place.map(|(_, server)| server);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Moderator).await
                        else {
                            continue;
                        };
                        let Some((channel_id, _)) = place else {
                            continue;
                        };

                        if let Ok(true) =
                            db::pin_message(&state.db, channel_id, message_id, uid).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::MessagePinned {
                                    channel_id,
                                    message_id,
                                    pinned_by: uid,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::UnpinMessage { message_id } => {
                        let Some(uid) = user_id else { continue };

                        let place = found(db::message_channel(&state.db, message_id).await);
                        let server = place.map(|(_, server)| server);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Moderator).await
                        else {
                            continue;
                        };
                        let Some((channel_id, _)) = place else {
                            continue;
                        };

                        if let Ok(true) = db::unpin_message(&state.db, channel_id, message_id).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::MessageUnpinned {
                                    channel_id,
                                    message_id,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::FetchPinnedMessages { channel_id } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::channel_server(&state.db, channel_id).await);
                        if authorize(&state, &sender, uid, server, Need::Member)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        match db::get_pinned_messages(&state.db, channel_id).await {
                            Ok(messages) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::PinnedMessages {
                                        channel_id,
                                        messages,
                                    },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Failed to fetch pinned messages: {}", e),
                        }
                    }

                    // ── Unread tracking ──
                    ClientEvent::AckMessage {
                        channel_id,
                        message_id,
                    } => {
                        let Some(uid) = user_id else { continue };

                        // The message must be in the channel it marks as read.
                        let server = found(db::message_channel(&state.db, message_id).await)
                            .filter(|(channel, _)| *channel == channel_id)
                            .map(|(_, server)| server);
                        if authorize(&state, &sender, uid, server, Need::Member)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        let _ = db::ack_message(&state.db, uid, channel_id, message_id).await;
                    }

                    // ── Server / channel management ──
                    ClientEvent::EditServer {
                        server_id,
                        name,
                        icon_url,
                    } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Moderator)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        let _ = db::edit_server(
                            &state.db,
                            server_id,
                            name.as_deref(),
                            icon_url.as_deref(),
                        )
                        .await;
                        broadcast_to_server(
                            &state,
                            server_id,
                            &ServerEvent::ServerUpdated {
                                server_id,
                                name,
                                icon_url,
                            },
                        )
                        .await;
                    }

                    ClientEvent::DeleteServer { server_id } => {
                        let Some(uid) = user_id else { continue };

                        // Founder only
                        if found(db::get_server_owner(&state.db, server_id).await) != Some(uid) {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Error {
                                    message: "Only the server founder can delete a server"
                                        .to_string(),
                                },
                            )
                            .await;
                            continue;
                        }

                        // Deleting the server deletes its member list, so read it first.
                        let members = listed(db::member_ids(&state.db, server_id).await);
                        if db::delete_server(&state.db, server_id).await.is_ok() {
                            send_to_users(
                                &state,
                                &members,
                                &ServerEvent::ServerDeleted { server_id },
                            )
                            .await;
                        }
                    }

                    ClientEvent::CreateCategory { server_id, name } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Moderator)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        match db::create_category(&state.db, server_id, &name).await {
                            Ok(category) => {
                                broadcast_to_server(
                                    &state,
                                    server_id,
                                    &ServerEvent::CategoryCreated {
                                        server_id,
                                        category,
                                    },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Failed to create category: {}", e),
                        }
                    }

                    ClientEvent::DeleteChannel { channel_id } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::channel_server(&state.db, channel_id).await);
                        if authorize(&state, &sender, uid, server, Need::Moderator)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        if let Ok(Some(server_id)) = db::delete_channel(&state.db, channel_id).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::ChannelDeleted {
                                    server_id,
                                    channel_id,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::EditChannel {
                        channel_id,
                        name,
                        topic,
                    } => {
                        let Some(uid) = user_id else { continue };

                        let server = found(db::channel_server(&state.db, channel_id).await);
                        let Some(server_id) =
                            authorize(&state, &sender, uid, server, Need::Moderator).await
                        else {
                            continue;
                        };

                        let _ = db::edit_channel(
                            &state.db,
                            channel_id,
                            name.as_deref(),
                            topic.as_deref(),
                        )
                        .await;
                        broadcast_to_server(
                            &state,
                            server_id,
                            &ServerEvent::ChannelUpdated {
                                channel_id,
                                name,
                                topic,
                            },
                        )
                        .await;
                    }

                    ClientEvent::LeaveServer { server_id } => {
                        let Some(uid) = user_id else { continue };

                        // Check not founder
                        if found(db::get_server_owner(&state.db, server_id).await) == Some(uid) {
                            let _ = send_event(&sender, &ServerEvent::Error {
                                message: "Server founder cannot leave. Transfer ownership or delete the server.".to_string(),
                            }).await;
                            continue;
                        }

                        // Read the members first, so the leaver hears it too.
                        let members = listed(db::member_ids(&state.db, server_id).await);
                        if let Ok(true) = db::leave_server(&state.db, uid, server_id).await {
                            send_to_users(
                                &state,
                                &members,
                                &ServerEvent::MemberLeft {
                                    server_id,
                                    user_id: uid,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::GetInviteCode { server_id } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Member)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        if let Ok(Some(invite_code)) =
                            db::get_invite_code(&state.db, server_id).await
                        {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::InviteCode {
                                    server_id,
                                    invite_code,
                                },
                            )
                            .await;
                        }
                    }

                    // ── User profile ──
                    ClientEvent::UpdateProfile {
                        display_name,
                        custom_status,
                    } => {
                        let Some(uid) = user_id else { continue };

                        let _ = db::update_profile(
                            &state.db,
                            uid,
                            display_name.as_deref(),
                            custom_status.as_deref(),
                        )
                        .await;
                        send_to_users(
                            &state,
                            &listed(db::co_member_ids(&state.db, uid).await),
                            &ServerEvent::ProfileUpdated {
                                user_id: uid,
                                display_name,
                                custom_status,
                            },
                        )
                        .await;
                    }

                    // ── DM history ──
                    ClientEvent::FetchDirectMessages {
                        friend_id,
                        before,
                        limit,
                    } => {
                        let Some(uid) = user_id else { continue };

                        let limit = limit.min(100);
                        match db::get_direct_messages(&state.db, uid, friend_id, before, limit)
                            .await
                        {
                            Ok(messages) => {
                                let has_more = messages.len() == limit as usize;
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::DirectMessageHistory {
                                        friend_id,
                                        messages,
                                        has_more,
                                    },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Failed to fetch DM history: {}", e),
                        }
                    }

                    // ── Search ──
                    ClientEvent::SearchMessages {
                        server_id,
                        channel_id,
                        query,
                        limit,
                    } => {
                        let Some(uid) = user_id else { continue };

                        // A channel search stays inside the server it names.
                        let scope = match channel_id {
                            Some(channel) => found(db::channel_server(&state.db, channel).await)
                                .filter(|server| *server == server_id),
                            None => Some(server_id),
                        };
                        if authorize(&state, &sender, uid, scope, Need::Member)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        let limit = limit.min(50);
                        match db::search_messages(&state.db, server_id, channel_id, &query, limit)
                            .await
                        {
                            Ok(messages) => {
                                let total_count = db::search_messages_count(
                                    &state.db, server_id, channel_id, &query,
                                )
                                .await
                                .unwrap_or(0);
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::SearchResults {
                                        query,
                                        messages,
                                        total_count,
                                    },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Search failed: {}", e),
                        }
                    }

                    // ── Custom emojis ──
                    ClientEvent::AddCustomEmoji {
                        server_id,
                        name,
                        image_url,
                    } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Moderator)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        match db::add_custom_emoji(&state.db, server_id, &name, &image_url, uid)
                            .await
                        {
                            Ok(emoji) => {
                                broadcast_to_server(
                                    &state,
                                    server_id,
                                    &ServerEvent::CustomEmojiAdded { server_id, emoji },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Failed to add custom emoji: {}", e),
                        }
                    }

                    ClientEvent::RemoveCustomEmoji {
                        server_id,
                        emoji_id,
                    } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Moderator)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        if let Ok(true) =
                            db::remove_custom_emoji(&state.db, server_id, emoji_id).await
                        {
                            broadcast_to_server(
                                &state,
                                server_id,
                                &ServerEvent::CustomEmojiRemoved {
                                    server_id,
                                    emoji_id,
                                },
                            )
                            .await;
                        }
                    }

                    ClientEvent::FetchCustomEmojis { server_id } => {
                        let Some(uid) = user_id else { continue };

                        if authorize(&state, &sender, uid, Some(server_id), Need::Member)
                            .await
                            .is_none()
                        {
                            continue;
                        }

                        match db::get_custom_emojis(&state.db, server_id).await {
                            Ok(emojis) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::CustomEmojiList { server_id, emojis },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Failed to fetch custom emojis: {}", e),
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

                        let target = match db::find_user_by_username(&state.db, &target_username)
                            .await
                        {
                            Ok(Some(u)) => u,
                            Ok(None) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: format!("User '{}' not found", target_username),
                                    },
                                )
                                .await;
                                continue;
                            }
                            Err(e) => {
                                tracing::error!("DB error looking up user: {}", e);
                                continue;
                            }
                        };

                        if target.id == uid {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Error {
                                    message: "Cannot add yourself".to_string(),
                                },
                            )
                            .await;
                            continue;
                        }

                        // Check if already friends
                        if db::are_friends(&state.db, uid, target.id)
                            .await
                            .unwrap_or(false)
                        {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Error {
                                    message: "Already friends".to_string(),
                                },
                            )
                            .await;
                            continue;
                        }

                        if let Err(e) = db::send_friend_request(&state.db, uid, target.id).await {
                            tracing::error!("Failed to send friend request: {}", e);
                            continue;
                        }

                        // Get sender info from DB and notify recipient
                        if let Ok(Some(from_user)) = sqlx::query_as::<_, db::UserRow>(
                            "SELECT id, username, display_name, avatar_url, github_id, status, custom_status, auth_token, public_key, created_at FROM users WHERE id = $1"
                        ).bind(uid).fetch_optional(&state.db).await {
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
                                tracing::info!(
                                    "Friend request accepted: {} <-> {}",
                                    from_user_id,
                                    uid
                                );
                            }
                            Ok(false) => {
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: "No pending request from this user".to_string(),
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to accept friend request: {}", e);
                            }
                        }
                    }

                    ClientEvent::DeclineFriendRequest { from_user_id } => {
                        let Some(uid) = user_id else { continue };
                        let _ = db::decline_friend_request(&state.db, from_user_id, uid).await;
                        send_to_user(
                            &state,
                            from_user_id,
                            &ServerEvent::FriendRequestDeclined { by_user_id: uid },
                        )
                        .await;
                    }

                    ClientEvent::FetchFriends => {
                        let Some(uid) = user_id else { continue };
                        match db::get_friends(&state.db, uid).await {
                            Ok(rows) => {
                                let friends: Vec<FriendPayload> = rows
                                    .into_iter()
                                    .map(|r| FriendPayload {
                                        user_id: r.user_id,
                                        username: r.username,
                                        display_name: r.display_name,
                                        avatar_url: r.avatar_url,
                                        public_key: r.public_key,
                                        status: parse_status(&r.status),
                                    })
                                    .collect();
                                let _ =
                                    send_event(&sender, &ServerEvent::FriendList { friends }).await;
                            }
                            Err(e) => tracing::error!("Failed to fetch friends: {}", e),
                        }
                    }

                    ClientEvent::FetchFriendRequests => {
                        let Some(uid) = user_id else { continue };
                        match db::get_pending_friend_requests(&state.db, uid).await {
                            Ok(rows) => {
                                let requests: Vec<FriendRequestPayload> = rows
                                    .into_iter()
                                    .map(|r| FriendRequestPayload {
                                        from_user_id: r.from_user_id,
                                        from_username: r.from_username,
                                        from_avatar_url: r.from_avatar_url,
                                        created_at: r.created_at,
                                    })
                                    .collect();
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::PendingFriendRequests { requests },
                                )
                                .await;
                            }
                            Err(e) => tracing::error!("Failed to fetch friend requests: {}", e),
                        }
                    }

                    ClientEvent::SendDirectMessage {
                        recipient_id,
                        encrypted_content,
                    } => {
                        let Some(uid) = user_id else { continue };

                        // Verify friendship
                        if !db::are_friends(&state.db, uid, recipient_id)
                            .await
                            .unwrap_or(false)
                        {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Error {
                                    message: "Not friends with this user".to_string(),
                                },
                            )
                            .await;
                            continue;
                        }

                        // Store and relay
                        match db::store_direct_message(
                            &state.db,
                            uid,
                            recipient_id,
                            &encrypted_content,
                        )
                        .await
                        {
                            Ok((msg_id, timestamp)) => {
                                // Get sender username
                                let sender_name = username_cache
                                    .clone()
                                    .unwrap_or_else(|| "Unknown".to_string());

                                // Relay to recipient if online
                                send_to_user(
                                    &state,
                                    recipient_id,
                                    &ServerEvent::DirectMessageReceived {
                                        from_user_id: uid,
                                        from_username: sender_name.clone(),
                                        encrypted_content: encrypted_content.clone(),
                                        timestamp,
                                        message_id: msg_id,
                                    },
                                )
                                .await;

                                // Echo back to sender for confirmation
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::DirectMessageReceived {
                                        from_user_id: uid,
                                        from_username: sender_name,
                                        encrypted_content,
                                        timestamp,
                                        message_id: msg_id,
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to store DM: {}", e);
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: "Failed to send message".to_string(),
                                    },
                                )
                                .await;
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
        announce_presence(&state, uid, artemis_core::models::user::UserStatus::Offline).await;

        tracing::info!("User {} disconnected", uid);
    }
}

/// The least role an event needs in the server it acts on.
#[derive(Clone, Copy, PartialEq)]
enum Need {
    Member,
    /// Founder or moderator.
    Moderator,
}

/// `server` when `user` holds `need` in it, otherwise None after telling the
/// socket why not. `server` is None when the event named a channel or message
/// that does not exist. That gets the same refusal as a server the user is not
/// in, so a non-member cannot probe which ids exist.
async fn authorize(
    state: &AppState,
    sender: &Arc<Mutex<SplitSink<WebSocket, WsMessage>>>,
    user: Uuid,
    server: Option<Uuid>,
    need: Need,
) -> Option<Uuid> {
    let role = match server {
        Some(server) => found(db::get_member_role(&state.db, user, server).await),
        None => None,
    };
    let refusal = match role.as_deref() {
        None => "Not a member of this server",
        Some("founder" | "moderator") => return server,
        Some(_) if need == Need::Member => return server,
        Some(_) => "Insufficient permissions",
    };
    let _ = send_event(
        sender,
        &ServerEvent::Error {
            message: refusal.to_string(),
        },
    )
    .await;
    None
}

/// The row a lookup found. A database error is logged and counts as not
/// found, so the event it guards is refused rather than let through.
fn found<T>(lookup: Result<Option<T>, sqlx::Error>) -> Option<T> {
    lookup.unwrap_or_else(|e| {
        tracing::error!("Database lookup failed: {}", e);
        None
    })
}

/// The users a lookup listed. A database error is logged and lists nobody, so
/// the event goes to no one rather than to the wrong people.
fn listed(lookup: Result<HashSet<Uuid>, sqlx::Error>) -> HashSet<Uuid> {
    lookup.unwrap_or_else(|e| {
        tracing::error!("Database lookup failed: {}", e);
        HashSet::new()
    })
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
    let json = serde_json::to_string(event)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
    sender
        .lock()
        .await
        .send(WsMessage::Text(json.into()))
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
    Ok(())
}

/// Send an event to every open socket of `users`.
async fn send_to_users(state: &AppState, users: &HashSet<Uuid>, event: &ServerEvent) {
    let json = match serde_json::to_string(event) {
        Ok(j) => j,
        Err(_) => return,
    };
    let connections = state.connections.read().await;
    for conn in connections.values() {
        if users.contains(&conn.user_id) {
            let _ = conn
                .sender
                .lock()
                .await
                .send(WsMessage::Text(json.clone().into()))
                .await;
        }
    }
}

/// Send an event to a specific user (if they're online).
async fn send_to_user(state: &AppState, target_user_id: Uuid, event: &ServerEvent) {
    send_to_users(state, &HashSet::from([target_user_id]), event).await;
}

/// Send an event to every member of `server_id`.
async fn broadcast_to_server(state: &AppState, server_id: Uuid, event: &ServerEvent) {
    let members = listed(db::member_ids(&state.db, server_id).await);
    send_to_users(state, &members, event).await;
}

/// Tell the people who share a server with `user` that their status changed,
/// and their friends through the friend list's own event. `user`'s own
/// sockets are not told.
async fn announce_presence(
    state: &AppState,
    user: Uuid,
    status: artemis_core::models::user::UserStatus,
) {
    let co_members = listed(db::co_member_ids(&state.db, user).await);
    let presence = ServerEvent::PresenceUpdate {
        user_id: user,
        status,
    };
    send_to_users(state, &co_members, &presence).await;

    let friends = db::get_friends(&state.db, user)
        .await
        .map(|rows| rows.into_iter().map(|friend| friend.user_id).collect());
    let presence = ServerEvent::FriendPresenceUpdate {
        user_id: user,
        status,
    };
    send_to_users(state, &listed(friends), &presence).await;
}

fn parse_status(s: &str) -> artemis_core::models::user::UserStatus {
    match s {
        "online" => artemis_core::models::user::UserStatus::Online,
        "idle" => artemis_core::models::user::UserStatus::Idle,
        "dnd" => artemis_core::models::user::UserStatus::DoNotDisturb,
        _ => artemis_core::models::user::UserStatus::Offline,
    }
}

#[cfg(test)]
mod tests;
