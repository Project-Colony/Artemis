use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use artemis_auth::GitHubOAuth;

use crate::db;
use crate::state::AppState;

pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/auth/github", get(github_login))
        .route("/auth/github/callback", get(github_callback))
        .route("/api/v1/servers", get(list_servers))
        .route("/api/v1/servers", post(create_server))
        .route("/api/v1/channels/{channel_id}/messages", get(list_messages))
        .route("/api/v1/me", get(get_me))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

// ── GitHub OAuth ──

async fn github_login(State(state): State<AppState>) -> impl IntoResponse {
    let oauth = GitHubOAuth::new(
        &state.github_client_id,
        &state.github_client_secret,
        &format!("{}/auth/github/callback", state.base_url),
    );

    use oauth2::CsrfToken;
    use oauth2::Scope;

    let (auth_url, _csrf_token) = oauth
        .client()
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("read:user".to_string()))
        .add_scope(Scope::new("user:email".to_string()))
        .url();

    Redirect::temporary(auth_url.as_str())
}

#[derive(Deserialize)]
struct GithubCallbackParams {
    code: String,
    #[allow(dead_code)]
    state: Option<String>,
}

async fn github_callback(
    State(state): State<AppState>,
    Query(params): Query<GithubCallbackParams>,
) -> impl IntoResponse {
    let oauth = GitHubOAuth::new(
        &state.github_client_id,
        &state.github_client_secret,
        &format!("{}/auth/github/callback", state.base_url),
    );

    use oauth2::basic::BasicTokenResponse;
    use oauth2::reqwest::async_http_client;
    use oauth2::{AuthorizationCode, TokenResponse};

    // Exchange code for token
    let token_result: Result<BasicTokenResponse, _> = oauth
        .client()
        .exchange_code(AuthorizationCode::new(params.code))
        .request_async(async_http_client)
        .await;

    let token: BasicTokenResponse = match token_result {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("OAuth token exchange failed: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "OAuth failed", "detail": e.to_string() })),
            )
                .into_response();
        }
    };

    let access_token = token.access_token().secret();

    // Fetch GitHub user profile
    let gh_user = match GitHubOAuth::fetch_user(access_token).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Failed to fetch GitHub user: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Failed to fetch user profile" })),
            )
                .into_response();
        }
    };

    // Generate auth token for Artemis
    let artemis_token = Uuid::new_v4().to_string();

    // Find or create user
    match db::find_user_by_github_id(&state.db, gh_user.id).await {
        Ok(Some(existing)) => {
            // Update token
            let _ = db::update_user_token(&state.db, existing.id, &artemis_token).await;
        }
        Ok(None) => {
            // Create new user
            let _ = db::create_user(
                &state.db,
                &gh_user.login,
                gh_user.name.as_deref(),
                gh_user.avatar_url.as_deref(),
                gh_user.id,
                &artemis_token,
            )
            .await;
        }
        Err(e) => {
            tracing::error!("DB error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Database error" })),
            )
                .into_response();
        }
    };

    // Return the token — the client will use this to connect via WebSocket
    Json(json!({
        "token": artemis_token,
        "github_username": gh_user.login,
        "avatar_url": gh_user.avatar_url,
    }))
    .into_response()
}

// ── API endpoints ──

#[derive(Deserialize)]
struct AuthHeader {
    token: String,
}

async fn get_me(
    State(state): State<AppState>,
    Query(auth): Query<AuthHeader>,
) -> impl IntoResponse {
    match db::find_user_by_token(&state.db, &auth.token).await {
        Ok(Some(user)) => Json(json!({
            "id": user.id,
            "username": user.username,
        }))
        .into_response(),
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid token" })),
        )
            .into_response(),
    }
}

async fn list_servers(
    State(state): State<AppState>,
    Query(auth): Query<AuthHeader>,
) -> impl IntoResponse {
    let user = match db::find_user_by_token(&state.db, &auth.token).await {
        Ok(Some(u)) => u,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid token" })),
            )
                .into_response()
        }
    };

    match db::get_user_servers(&state.db, user.id).await {
        Ok(servers) => Json(json!({ "servers": servers })).into_response(),
        Err(e) => {
            tracing::error!("Failed to list servers: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "DB error" })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateServerBody {
    name: String,
}

async fn create_server(
    State(state): State<AppState>,
    Query(auth): Query<AuthHeader>,
    Json(body): Json<CreateServerBody>,
) -> impl IntoResponse {
    let user = match db::find_user_by_token(&state.db, &auth.token).await {
        Ok(Some(u)) => u,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid token" })),
            )
                .into_response()
        }
    };

    let invite_code: String = Uuid::new_v4().to_string()[..8].to_string();
    match db::create_server(&state.db, &body.name, user.id, &invite_code).await {
        Ok(server_id) => Json(json!({
            "server_id": server_id,
            "invite_code": invite_code,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to create server: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "DB error" })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct MessageQuery {
    token: String,
    before: Option<Uuid>,
    limit: Option<u32>,
}

async fn list_messages(
    State(state): State<AppState>,
    Path(channel_id): Path<Uuid>,
    Query(query): Query<MessageQuery>,
) -> impl IntoResponse {
    let _user = match db::find_user_by_token(&state.db, &query.token).await {
        Ok(Some(u)) => u,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid token" })),
            )
                .into_response()
        }
    };

    let limit = query.limit.unwrap_or(50).min(100);
    match db::get_messages(&state.db, channel_id, query.before, limit).await {
        Ok(messages) => Json(json!({ "messages": messages })).into_response(),
        Err(e) => {
            tracing::error!("Failed to list messages: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "DB error" })),
            )
                .into_response()
        }
    }
}
