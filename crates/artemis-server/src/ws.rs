use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::{Sink, SinkExt, StreamExt};
use tokio::sync::{mpsc, OwnedSemaphorePermit, RwLock};
use tokio::task::AbortHandle;
use tokio::time::{interval_at, sleep, timeout, Instant};
use uuid::Uuid;

use artemis_core::protocol::{ClientEvent, FriendPayload, FriendRequestPayload, ServerEvent};

use crate::db;
use crate::state::AppState;

pub fn ws_routes() -> Router<AppState> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    // The socket holds the permit until it signs in, so the cap counts open
    // sockets rather than handshakes in flight.
    // ponytail: one cap for every address. One host that keeps reopening
    // silent sockets holds all MAX_UNAUTHENTICATED places and refuses every
    // new connection for as long as it keeps going. No per-IP cap, because
    // behind a proxy every peer shares one address; add ConnectInfo plus a
    // trusted X-Forwarded-For per-IP cap if that appears.
    let Ok(permit) = state.unauthenticated.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| handle_socket(socket, state, permit))
}

async fn handle_socket(socket: WebSocket, state: AppState, permit: OwnedSemaphorePermit) {
    // Released once the socket signs in.
    let mut unauthenticated = Some(permit);
    let deadline = sleep(AUTH_DEADLINE);
    tokio::pin!(deadline);
    let mut bucket = EventBucket::new();
    let (sink, mut receiver) = socket.split();
    // Every frame to this socket, the replies below and the fan-out alike,
    // goes through this queue, so they reach the client in the order sent.
    let (sender, queue) = mpsc::channel(SEND_QUEUE);
    let ponged = Arc::new(AtomicBool::new(true));
    let mut writer = tokio::spawn(write_frames(sink, queue, ponged.clone()));

    let mut user_id: Option<Uuid> = None;
    // A socket gets one Authenticate.
    let mut tried_to_sign_in = false;
    let mut username_cache: Option<String> = None;
    let conn_id = Uuid::new_v4();

    loop {
        let msg = tokio::select! {
            msg = receiver.next() => match msg {
                Some(Ok(msg)) => msg,
                _ => break,
            },
            // The writer gave up on the socket, or the fan-out dropped it.
            _ = &mut writer => break,
            _ = &mut deadline, if user_id.is_none() => {
                tracing::debug!("Closing a socket that did not sign in in time");
                break;
            }
        };
        if !bucket.take() {
            tracing::warn!("Closing a socket that sent events too fast");
            break;
        }
        match msg {
            WsMessage::Text(txt) => {
                let event = serde_json::from_str::<ClientEvent>(&txt);
                if user_id.is_none() {
                    // A refused socket ignores what the client sent after its
                    // Authenticate until the deadline closes it. Closing at
                    // once would abort the writer before the refusal goes out.
                    if tried_to_sign_in {
                        continue;
                    }
                    // Before that, anything but Authenticate closes it.
                    if !matches!(event, Ok(ClientEvent::Authenticate { .. })) {
                        break;
                    }
                    tried_to_sign_in = true;
                }
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        // The error can quote the frame, so it is never sent back.
                        tracing::debug!("Invalid event: {}", e);
                        let _ = send_event(
                            &sender,
                            &ServerEvent::Error {
                                message: "Invalid event".to_string(),
                            },
                        )
                        .await;
                        continue;
                    }
                };

                match event {
                    ClientEvent::Authenticate { token } => {
                        if user_id.is_some() {
                            let _ = send_event(
                                &sender,
                                &ServerEvent::Error {
                                    message: "Already authenticated".to_string(),
                                },
                            )
                            .await;
                            continue;
                        }
                        match db::find_user_by_token(&state.db, &token).await {
                            Ok(Some(user)) => {
                                // Register the socket unless the user has as
                                // many open as they may. The user's first one
                                // brings them online; later ones keep the
                                // status they already have, Idle or DND too.
                                let presence = state.presence.lock().await;
                                let first = {
                                    let mut connections = state.connections.write().await;
                                    let open = connections
                                        .values()
                                        .filter(|conn| conn.user_id == user.id)
                                        .count();
                                    (open < MAX_SOCKETS_PER_USER).then(|| {
                                        connections.insert(
                                            conn_id,
                                            ConnectedUser {
                                                user_id: user.id,
                                                queue: sender.clone(),
                                                writer: writer.abort_handle(),
                                            },
                                        );
                                        open == 0
                                    })
                                };
                                let Some(first) = first else {
                                    drop(presence);
                                    let _ = send_event(
                                        &sender,
                                        &ServerEvent::AuthError {
                                            reason: "Too many connections for this account"
                                                .to_string(),
                                        },
                                    )
                                    .await;
                                    continue;
                                };
                                user_id = Some(user.id);
                                username_cache = Some(user.username.clone());
                                drop(unauthenticated.take());
                                if first {
                                    let _ =
                                        db::update_user_status(&state.db, user.id, "online").await;
                                    announce_presence(
                                        &state,
                                        user.id,
                                        artemis_core::models::user::UserStatus::Online,
                                    )
                                    .await;
                                }
                                drop(presence);

                                // Fetch user's servers
                                let servers = db::get_user_servers(&state.db, user.id)
                                    .await
                                    .unwrap_or_default();

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
                        let Some(uid) = user_id else { continue };

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
                            // The user's other devices show it too.
                            send_to_user(
                                &state,
                                uid,
                                &ServerEvent::PresenceUpdate {
                                    user_id: uid,
                                    status,
                                },
                            )
                            .await;
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

                        match db::delete_server(&state.db, server_id).await {
                            Ok(members) => {
                                send_to_users(
                                    &state,
                                    &members,
                                    &ServerEvent::ServerDeleted { server_id },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to delete server: {}", e);
                                let _ = send_event(
                                    &sender,
                                    &ServerEvent::Error {
                                        message: "Failed to delete server".to_string(),
                                    },
                                )
                                .await;
                            }
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

                        // The members include the leaver, so they hear it too.
                        let members = listed(db::leave_server(&state.db, uid, server_id).await);
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
                        // Co-members, and the user's own devices.
                        let mut recipients = listed(db::co_member_ids(&state.db, uid).await);
                        recipients.insert(uid);
                        send_to_users(
                            &state,
                            &recipients,
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
            WsMessage::Pong(_) => ponged.store(true, Ordering::Relaxed),
            WsMessage::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup on disconnect. Only a signed-in socket is in the map.
    writer.abort();
    let Some(uid) = user_id else {
        return;
    };
    // Under the same lock as sign-in, so a sign-in racing this close cannot
    // be followed by this socket's offline write.
    let _presence = state.presence.lock().await;
    let still_connected = {
        let mut connections = state.connections.write().await;
        connections.remove(&conn_id);
        connections.values().any(|conn| conn.user_id == uid)
    };
    // The user stays online while another of their sockets is open.
    if !still_connected {
        let _ = db::update_user_status(&state.db, uid, "offline").await;
        announce_presence(&state, uid, artemis_core::models::user::UserStatus::Offline).await;
    }
    tracing::info!("User {} disconnected", uid);
}

/// The largest frame, and the largest message, a client may send. A frame
/// over it closes the socket before its payload is read.
const MAX_MESSAGE: usize = 64 * 1024;
/// How long a new socket has to sign in before it is closed.
const AUTH_DEADLINE: Duration = Duration::from_secs(10);
/// Sockets open at once that have not signed in yet.
pub const MAX_UNAUTHENTICATED: usize = 512;
/// Sockets one user may have signed in at once.
const MAX_SOCKETS_PER_USER: usize = 5;
/// Frames a socket may send each second, on average.
const EVENTS_PER_SECOND: f64 = 20.0;
/// Frames a socket may send at once after a quiet spell.
const EVENT_BURST: f64 = 40.0;

/// A token bucket over the frames one socket sends. A socket that empties it
/// is closed.
struct EventBucket {
    tokens: f64,
    last: Instant,
}

impl EventBucket {
    fn new() -> Self {
        Self {
            tokens: EVENT_BURST,
            last: Instant::now(),
        }
    }

    /// Takes a token for one frame, or returns false when none is left.
    fn take(&mut self) -> bool {
        let now = Instant::now();
        let refill = now.duration_since(self.last).as_secs_f64() * EVENTS_PER_SECOND;
        self.tokens = (self.tokens + refill).min(EVENT_BURST);
        self.last = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// Frames a socket may have waiting. Enough for a burst of events; a client
/// that falls this far behind is dropped by the fan-out.
const SEND_QUEUE: usize = 256;
/// The longest one frame may take to go out before the socket is dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the relay pings a socket. A socket that has not answered the
/// previous ping by the next one is dropped.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Writes a socket's queued frames and pings it. Stops when the queue closes,
/// when a frame takes longer than SEND_TIMEOUT, or when a ping got no pong.
async fn write_frames(
    mut sink: impl Sink<WsMessage> + Unpin,
    mut queue: mpsc::Receiver<Utf8Bytes>,
    ponged: Arc<AtomicBool>,
) {
    let mut ping = interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    loop {
        let frame = tokio::select! {
            text = queue.recv() => match text {
                Some(text) => WsMessage::Text(text),
                None => return,
            },
            _ = ping.tick() => {
                if !ponged.swap(false, Ordering::Relaxed) {
                    return;
                }
                WsMessage::Ping(Default::default())
            }
        };
        if !matches!(timeout(SEND_TIMEOUT, sink.send(frame)).await, Ok(Ok(()))) {
            return;
        }
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
    sender: &mpsc::Sender<Utf8Bytes>,
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
    /// The socket's send queue.
    pub queue: mpsc::Sender<Utf8Bytes>,
    /// Stops the socket's writer, which ends its connection.
    pub writer: AbortHandle,
}

pub type ConnectionMap = Arc<RwLock<HashMap<Uuid, ConnectedUser>>>;

pub fn new_connection_map() -> ConnectionMap {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Queue an event for this socket, behind everything queued before it. Waits
/// while the queue is full, which only ever holds up this socket's own reader.
async fn send_event(
    sender: &mpsc::Sender<Utf8Bytes>,
    event: &ServerEvent,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sender.send(serde_json::to_string(event)?.into()).await?;
    Ok(())
}

/// Send an event to every open socket of `users`. The queues are copied out
/// of the map first, so no send runs under its lock. A socket whose queue is
/// full has fallen behind and is dropped rather than waited for.
async fn send_to_users(state: &AppState, users: &HashSet<Uuid>, event: &ServerEvent) {
    let Ok(json) = serde_json::to_string(event) else {
        return;
    };
    let text = Utf8Bytes::from(json);
    let queues: Vec<(Uuid, mpsc::Sender<Utf8Bytes>)> = state
        .connections
        .read()
        .await
        .iter()
        .filter(|(_, conn)| users.contains(&conn.user_id))
        .map(|(id, conn)| (*id, conn.queue.clone()))
        .collect();
    let behind: Vec<Uuid> = queues
        .into_iter()
        .filter(|(_, queue)| queue.try_send(text.clone()).is_err())
        .map(|(id, _)| id)
        .collect();
    if behind.is_empty() {
        return;
    }
    let mut connections = state.connections.write().await;
    for id in behind {
        if let Some(conn) = connections.remove(&id) {
            tracing::warn!(
                "Dropping a socket of user {} that fell behind",
                conn.user_id
            );
            conn.writer.abort();
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
/// sockets are not told: on sign-in and disconnect they have nothing to
/// update, and `UpdatePresence` tells them itself.
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
