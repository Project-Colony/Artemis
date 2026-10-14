//! The client's side of the relay: signing in over HTTP, then a session
//! socket that reconnects on its own.

use std::time::Duration;

use futures::channel::mpsc::Sender;
use futures::{SinkExt, Stream, StreamExt};
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use artemis_core::protocol::{ClientEvent, DeviceCode, DevicePoll, ServerEvent};

use crate::AppMessage;

/// Where the user enters the sign-in code. The client opens this page itself,
/// never the `verification_uri` a relay sends, so a relay cannot send its
/// users to another page.
pub const GITHUB_DEVICE_PAGE: &str = "https://github.com/login/device";

/// The wait before the first reconnection, doubled after each failed one.
const FIRST_RETRY: Duration = Duration::from_secs(1);
/// The longest wait between two reconnections.
const LAST_RETRY: Duration = Duration::from_secs(30);

/// Signs in through the relay's device flow. Yields `DeviceCodeReceived` once
/// the relay has a code for the user, then `SignedIn` with the relay's session
/// token, or `DeviceFlowError`. The client never sees a GitHub token.
pub fn sign_in(relay: Url) -> impl Stream<Item = AppMessage> {
    iced::stream::channel(4, move |mut output: Sender<AppMessage>| async move {
        let outcome = device_flow(&relay, &mut output)
            .await
            .unwrap_or_else(AppMessage::DeviceFlowError);
        let _ = output.send(outcome).await;
    })
}

async fn device_flow(relay: &Url, output: &mut Sender<AppMessage>) -> Result<AppMessage, String> {
    let http = reqwest::Client::builder()
        // A redirect could send the device code elsewhere.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let code: DeviceCode = post(&http, relay, "/auth/device/start", json!({})).await?;
    let _ = output
        .send(AppMessage::DeviceCodeReceived {
            user_code: code.user_code,
        })
        .await;

    // GitHub asks for 5 seconds or more, which the relay passes on. The
    // floor only keeps a relay that sends 0 from being polled in a loop.
    let mut every = Duration::from_secs(code.interval.max(1));
    let poll = json!({ "device_code": code.device_code });
    loop {
        tokio::time::sleep(every).await;
        match post(&http, relay, "/auth/device/poll", poll.clone()).await? {
            DevicePoll::Pending => {}
            DevicePoll::SlowDown { interval } => {
                every = interval.map_or(every + Duration::from_secs(5), |secs| {
                    Duration::from_secs(secs.max(1))
                });
            }
            DevicePoll::Denied => return Err("Access was denied on GitHub.".to_string()),
            DevicePoll::Expired => return Err("The code expired. Try again.".to_string()),
            DevicePoll::Complete {
                token,
                username,
                avatar_url,
            } => {
                return Ok(AppMessage::SignedIn {
                    token,
                    username,
                    avatar_url,
                })
            }
        }
    }
}

/// POSTs `body` to `path` on the relay. A refusal carries `{"error": "..."}`.
async fn post<T: DeserializeOwned>(
    http: &reqwest::Client,
    relay: &Url,
    path: &str,
    body: Value,
) -> Result<T, String> {
    let url = relay.join(path).map_err(|e| e.to_string())?;
    let response = http
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Cannot reach the relay: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|e| format!("The relay sent an answer Artemis does not understand: {e}"));
    }
    #[derive(Deserialize)]
    struct Refusal {
        error: String,
    }
    Err(match response.json::<Refusal>().await {
        Ok(refusal) => format!("The relay refused to sign you in: {}", refusal.error),
        Err(_) => format!("The relay answered {status}"),
    })
}

/// The relay's WebSocket endpoint: `/ws` on the same host, over wss for an
/// https relay and ws for an http one.
fn ws_url(relay: &Url) -> Result<Url, String> {
    let scheme = match relay.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => {
            return Err(format!(
                "A relay address starts with https:// or http://, not {other}://"
            ))
        }
    };
    let mut url = relay.join("/ws").map_err(|e| e.to_string())?;
    url.set_scheme(scheme)
        .map_err(|()| format!("Cannot use {scheme} for {relay}"))?;
    Ok(url)
}

/// Keeps a session with the relay. Each socket signs in with `token` and
/// passes events both ways, announced by `Connected` with the sender for
/// events to the relay. When a socket ends, the stream yields `Disconnected`
/// and opens a new one, after 1 second at first and twice as long after each
/// failed attempt, up to 30 seconds. It runs until the app drops it.
pub fn session(relay: Url, token: String) -> impl Stream<Item = AppMessage> {
    iced::stream::channel(64, move |mut output: Sender<AppMessage>| async move {
        let url = match ws_url(&relay) {
            Ok(url) => url,
            Err(e) => {
                let _ = output.send(AppMessage::ConnectionFailed(e)).await;
                return;
            }
        };
        let mut retry = FIRST_RETRY;
        loop {
            let ended = match tokio_tungstenite::connect_async(url.as_str()).await {
                Ok((socket, _)) => {
                    retry = FIRST_RETRY;
                    let (tx, rx) = mpsc::unbounded_channel();
                    if output.send(AppMessage::Connected(tx)).await.is_err() {
                        return;
                    }
                    AppMessage::Disconnected(run(socket, &token, rx, &mut output).await)
                }
                Err(e) => AppMessage::ConnectionFailed(e.to_string()),
            };
            if output.send(ended).await.is_err() {
                return;
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(LAST_RETRY);
        }
    })
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Signs `socket` in with `token`, then passes events both ways until it
/// ends. Returns why it ended.
async fn run(
    socket: Socket,
    token: &str,
    mut outgoing: mpsc::UnboundedReceiver<ClientEvent>,
    output: &mut Sender<AppMessage>,
) -> String {
    let (mut sink, mut frames) = socket.split();
    let mut next = Some(ClientEvent::Authenticate {
        token: token.to_string(),
    });
    loop {
        if let Some(event) = next.take() {
            let json = serde_json::to_string(&event).expect("client events always serialize");
            if let Err(e) = sink.send(WsMessage::Text(json.into())).await {
                return e.to_string();
            }
        }
        tokio::select! {
            event = outgoing.recv() => match event {
                Some(event) => next = Some(event),
                None => return "The app closed the connection".to_string(),
            },
            frame = frames.next() => match frame {
                Some(Ok(WsMessage::Text(text))) => {
                    match serde_json::from_str::<ServerEvent>(text.as_str()) {
                        Ok(event) => {
                            if output.send(AppMessage::ServerEventReceived(event)).await.is_err() {
                                return "The app closed the connection".to_string();
                            }
                        }
                        Err(e) => tracing::warn!("Ignoring an event from the relay: {e}"),
                    }
                }
                Some(Ok(WsMessage::Close(_))) | None => {
                    return "The relay closed the connection".to_string();
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return e.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn the_socket_url_follows_the_relay_scheme() {
        let ws = |relay: &str| ws_url(&relay.parse().unwrap()).map(String::from);
        // A plain text replace of "http" would also rewrite the host.
        assert_eq!(
            ws("https://http-relay.example.org").unwrap(),
            "wss://http-relay.example.org/ws"
        );
        assert_eq!(
            ws("http://localhost:3000").unwrap(),
            "ws://localhost:3000/ws"
        );
        assert!(ws("ftp://relay.example.org").is_err());
    }

    /// The first frame of each socket, which the stub relay answers with
    /// Authenticated before it closes the socket.
    async fn first_frame(mut socket: WebSocket, frames: Arc<Mutex<Vec<Value>>>) {
        if let Some(Ok(Message::Text(text))) = socket.recv().await {
            frames
                .lock()
                .unwrap()
                .push(serde_json::from_str(text.as_str()).unwrap());
        }
        let authenticated = json!({"type": "Authenticated", "data": {
            "user_id": Uuid::nil(), "username": "ada", "servers": []
        }});
        let _ = socket
            .send(Message::Text(authenticated.to_string().into()))
            .await;
        let _ = socket.send(Message::Close(None)).await;
    }

    /// A relay on a loopback port. Its device flow grants code "dc" on the
    /// second poll with the session "relay-session", and its sockets record
    /// their first frame in `frames`.
    async fn stub_relay(frames: Arc<Mutex<Vec<Value>>>) -> Url {
        let polls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/auth/device/start",
                post(|| async {
                    Json(json!({
                        "device_code": "dc",
                        "user_code": "WDJB-MJHT",
                        // Never opened: the client opens GitHub's page itself.
                        "verification_uri": "https://relay.example.org/elsewhere",
                        "expires_in": 900,
                        "interval": 1
                    }))
                }),
            )
            .route(
                "/auth/device/poll",
                post(move |Json(body): Json<Value>| async move {
                    if body != json!({ "device_code": "dc" }) {
                        let refusal = json!({ "error": "Invalid device code" });
                        return (StatusCode::BAD_REQUEST, Json(refusal));
                    }
                    let answer = match polls.fetch_add(1, Ordering::SeqCst) {
                        0 => json!({ "status": "pending" }),
                        _ => json!({
                            "status": "complete",
                            "token": "relay-session",
                            "username": "ada",
                            "avatar_url": null
                        }),
                    };
                    (StatusCode::OK, Json(answer))
                }),
            )
            .route(
                "/ws",
                get(move |ws: WebSocketUpgrade| {
                    let frames = Arc::clone(&frames);
                    async move { ws.on_upgrade(move |socket| first_frame(socket, frames)) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        url.parse().unwrap()
    }

    #[tokio::test]
    async fn signs_in_through_the_relay_and_opens_every_socket_with_its_session() {
        let frames = Arc::default();
        let relay = stub_relay(Arc::clone(&frames)).await;

        let signed_in: Vec<AppMessage> = sign_in(relay.clone()).collect().await;
        assert!(
            matches!(
                &signed_in[..],
                [
                    AppMessage::DeviceCodeReceived { user_code },
                    AppMessage::SignedIn { token, username, .. },
                ] if user_code == "WDJB-MJHT" && token == "relay-session" && username == "ada"
            ),
            "{signed_in:?}"
        );

        let http = reqwest::Client::new();
        let refused =
            super::post::<DevicePoll>(&http, &relay, "/auth/device/poll", json!({})).await;
        assert_eq!(
            refused.unwrap_err(),
            "The relay refused to sign you in: Invalid device code"
        );

        // The stub closes each socket after one event, and the session opens
        // a new one.
        let mut session = Box::pin(session(relay, "relay-session".to_string()));
        for _ in 0..2 {
            let Some(AppMessage::Connected(_relay_tx)) = session.next().await else {
                panic!("not connected");
            };
            assert!(matches!(
                session.next().await,
                Some(AppMessage::ServerEventReceived(
                    ServerEvent::Authenticated { .. }
                ))
            ));
            assert!(matches!(
                session.next().await,
                Some(AppMessage::Disconnected(_))
            ));
        }
        // Each socket signed in with the relay's session and nothing else.
        let authenticate = json!({"type": "Authenticate", "data": {"token": "relay-session"}});
        assert_eq!(
            *frames.lock().unwrap(),
            [authenticate.clone(), authenticate]
        );
    }
}
