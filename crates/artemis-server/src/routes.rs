use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
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

    let (auth_url, csrf_token) = oauth
        .client()
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("read:user".to_string()))
        .add_scope(Scope::new("user:email".to_string()))
        .url();

    // The callback only accepts the `state` this browser was given here.
    let secure = if state.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{OAUTH_STATE_COOKIE}={}; Path=/auth/github; HttpOnly; SameSite=Lax; Max-Age=600{secure}",
        csrf_token.secret()
    );
    (
        [(header::SET_COOKIE, cookie)],
        Redirect::temporary(auth_url.as_str()),
    )
}

/// Cookie that ties an OAuth callback to the browser that started the login.
const OAUTH_STATE_COOKIE: &str = "artemis_oauth_state";

/// Whether the `state` GitHub sent back is the one stored in this browser's
/// cookie. Without this check, anyone could send a victim a callback link
/// carrying the attacker's own authorization code (login CSRF).
fn oauth_state_matches(headers: &HeaderMap, returned: Option<&str>) -> bool {
    let stored = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            pair.trim()
                .strip_prefix(OAUTH_STATE_COOKIE)?
                .strip_prefix('=')
        });
    matches!((stored, returned), (Some(stored), Some(returned)) if !stored.is_empty() && stored == returned)
}

#[derive(Deserialize)]
struct GithubCallbackParams {
    code: String,
    state: Option<String>,
}

async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<GithubCallbackParams>,
) -> impl IntoResponse {
    if !oauth_state_matches(&headers, params.state.as_deref()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "OAuth state mismatch" })),
        )
            .into_response();
    }

    let oauth = GitHubOAuth::new(
        &state.github_client_id,
        &state.github_client_secret,
        &format!("{}/auth/github/callback", state.base_url),
    );

    use oauth2::basic::BasicTokenResponse;
    use oauth2::{AuthorizationCode, TokenResponse};

    // No redirects: the oauth2 crate recommends this to keep the token request
    // from being bounced to another host (SSRF).
    let http_client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => artemis_auth::OAuthHttp(c),
        Err(e) => {
            tracing::error!("Failed to build the HTTP client: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "OAuth failed" })),
            )
                .into_response();
        }
    };

    // Exchange code for token
    let token_result: Result<BasicTokenResponse, _> = oauth
        .client()
        .exchange_code(AuthorizationCode::new(params.code))
        .request_async(&http_client)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cookies(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, value.parse().unwrap());
        headers
    }

    #[test]
    fn oauth_state_must_match_the_cookie() {
        let headers = cookies("theme=dark; artemis_oauth_state=abc123");
        assert!(oauth_state_matches(&headers, Some("abc123")));
        assert!(!oauth_state_matches(&headers, Some("other")));
        assert!(!oauth_state_matches(&headers, None));
        assert!(!oauth_state_matches(&HeaderMap::new(), Some("abc123")));
        assert!(!oauth_state_matches(
            &cookies("artemis_oauth_state="),
            Some("")
        ));
        assert!(!oauth_state_matches(
            &cookies("artemis_oauth_state_x=abc123"),
            Some("abc123")
        ));
    }
}
