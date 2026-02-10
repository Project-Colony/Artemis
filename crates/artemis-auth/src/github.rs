use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, ClientId, ClientSecret, RedirectUrl, TokenUrl};
use serde::{Deserialize, Serialize};

const GITHUB_AUTH_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_API: &str = "https://api.github.com/user";

/// GitHub user profile returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
}

/// GitHub OAuth client configuration.
pub struct GitHubOAuth {
    client: BasicClient,
}

impl GitHubOAuth {
    /// Create a new GitHub OAuth client.
    ///
    /// - `client_id`: GitHub OAuth App client ID
    /// - `client_secret`: GitHub OAuth App client secret
    /// - `redirect_url`: Callback URL (e.g. `http://localhost:3000/auth/github/callback`)
    pub fn new(client_id: &str, client_secret: &str, redirect_url: &str) -> Self {
        let client = BasicClient::new(
            ClientId::new(client_id.to_string()),
            Some(ClientSecret::new(client_secret.to_string())),
            AuthUrl::new(GITHUB_AUTH_URL.to_string()).unwrap(),
            Some(TokenUrl::new(GITHUB_TOKEN_URL.to_string()).unwrap()),
        )
        .set_redirect_uri(RedirectUrl::new(redirect_url.to_string()).unwrap());

        Self { client }
    }

    /// Get the OAuth2 client (for generating auth URLs, exchanging codes, etc.)
    pub fn client(&self) -> &BasicClient {
        &self.client
    }

    /// Fetch the authenticated GitHub user profile using an access token.
    pub async fn fetch_user(access_token: &str) -> Result<GitHubUser, reqwest::Error> {
        let client = reqwest::Client::new();
        client
            .get(GITHUB_USER_API)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("User-Agent", "Artemis")
            .send()
            .await?
            .json::<GitHubUser>()
            .await
    }
}
