use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use artemis_auth::DeviceFlowError;
use artemis_core::protocol::DevicePoll;

use crate::db;
use crate::state::AppState;

pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/auth/device/start", post(device_start))
        .route("/auth/device/poll", post(device_poll))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

// ── Sign-in through GitHub's device flow ──

/// Asks GitHub for a code the user enters at its verification page. The
/// answer holds `device_code`, `user_code`, `verification_uri`, `interval`
/// and `expires_in`.
// ponytail: no rate limit on start or poll. Each call is one request to
// GitHub, so anyone can spend the OAuth app's quota there and block sign-ins
// while it lasts. Add a per-IP limit to both if that appears.
async fn device_start(State(state): State<AppState>) -> Response {
    match state.github.request_device_code().await {
        Ok(code) => Json(code).into_response(),
        Err(e) => not_signed_in(e),
    }
}

#[derive(Deserialize)]
struct PollRequest {
    device_code: String,
}

/// Asks GitHub once whether the user has entered the code. Once they have,
/// GitHub's token is used for one profile read and dropped, never stored or
/// logged, and the user gets a relay session instead.
async fn device_poll(State(state): State<AppState>, Json(request): Json<PollRequest>) -> Response {
    // GitHub's device codes are 40 hex characters. Anything far from that is
    // refused here instead of being sent on.
    let code = &request.device_code;
    if code.is_empty() || code.len() > 64 || !code.bytes().all(|b| b.is_ascii_graphic()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid device code" })),
        )
            .into_response();
    }
    let profile = match state.github.poll_device_token(&request.device_code).await {
        Ok(access_token) => state.github.fetch_user(&access_token).await,
        Err(e) => Err(e),
    };
    let profile = match profile {
        Ok(profile) => profile,
        Err(e) => return not_signed_in(e),
    };
    match db::sign_in(&state.db, &profile).await {
        Ok((user, token)) => Json(DevicePoll::Complete {
            token,
            username: user.username,
            avatar_url: user.avatar_url,
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Cannot sign in GitHub user {}: {e}", profile.id);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Database error" })),
            )
                .into_response()
        }
    }
}

/// The answer to a device flow request that gave no profile: one of the
/// states the client keeps polling through, or 502 when GitHub refused.
fn not_signed_in(e: DeviceFlowError) -> Response {
    let state = match e {
        DeviceFlowError::Pending => DevicePoll::Pending,
        DeviceFlowError::SlowDown { interval } => DevicePoll::SlowDown { interval },
        DeviceFlowError::Denied => DevicePoll::Denied,
        DeviceFlowError::Expired => DevicePoll::Expired,
        e => {
            tracing::warn!("GitHub sign-in failed: {e}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "GitHub sign-in failed" })),
            )
                .into_response();
        }
    };
    Json(state).into_response()
}
