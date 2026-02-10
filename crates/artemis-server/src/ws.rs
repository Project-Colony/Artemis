use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

pub fn ws_routes() -> Router {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    tracing::info!("New WebSocket connection");

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(txt) => {
                tracing::debug!("Received: {}", txt);
                // Echo back for now — will be replaced with proper protocol
                if socket.send(Message::Text(txt)).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => {
                tracing::info!("Client disconnected");
                break;
            }
            _ => {}
        }
    }
}
