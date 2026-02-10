use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use artemis_core::protocol::{ClientEvent, ServerEvent};

/// Connect to the Artemis WebSocket server and return channels for sending/receiving events.
pub async fn connect(
    server_url: &str,
    token: String,
) -> Result<
    (
        mpsc::UnboundedSender<ClientEvent>,
        mpsc::UnboundedReceiver<ServerEvent>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let ws_url = format!("{}/ws", server_url.replace("http", "ws"));
    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Channel: app -> server
    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<ClientEvent>();
    // Channel: server -> app
    let (server_tx, server_rx) = mpsc::unbounded_channel::<ServerEvent>();

    // Send auth immediately
    let auth_event = ClientEvent::Authenticate { token };
    let auth_json = serde_json::to_string(&auth_event)?;
    ws_sink.send(WsMessage::Text(auth_json.into())).await?;

    // Task: forward client events to WS
    tokio::spawn(async move {
        while let Some(event) = client_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                if ws_sink.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Task: forward WS events to app
    tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_source.next().await {
            if let WsMessage::Text(txt) = msg {
                if let Ok(event) = serde_json::from_str::<ServerEvent>(&txt) {
                    if server_tx.send(event).is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok((client_tx, server_rx))
}
