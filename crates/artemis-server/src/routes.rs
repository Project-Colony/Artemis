use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

pub fn api_routes() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/servers", get(list_servers))
        .route("/api/v1/channels/{channel_id}/messages", get(list_messages))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

async fn list_servers() -> impl IntoResponse {
    // TODO: fetch from database
    (StatusCode::OK, Json(json!({ "servers": [] })))
}

async fn list_messages() -> impl IntoResponse {
    // TODO: fetch from database
    (StatusCode::OK, Json(json!({ "messages": [] })))
}
