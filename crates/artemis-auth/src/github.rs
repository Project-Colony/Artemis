use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl, TokenUrl};
use serde::{Deserialize, Serialize};

const GITHUB_AUTH_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_API: &str = "https://api.github.com/user";
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";

/// GitHub user profile returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
}

/// An OAuth client with the authorization and token endpoints set.
pub type GitHubClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// GitHub OAuth client configuration (for server-side flow).
pub struct GitHubOAuth {
    client: GitHubClient,
}

impl GitHubOAuth {
    pub fn new(client_id: &str, client_secret: &str, redirect_url: &str) -> Self {
        let client = BasicClient::new(ClientId::new(client_id.to_string()))
            .set_client_secret(ClientSecret::new(client_secret.to_string()))
            .set_auth_uri(AuthUrl::new(GITHUB_AUTH_URL.to_string()).unwrap())
            .set_token_uri(TokenUrl::new(GITHUB_TOKEN_URL.to_string()).unwrap())
            .set_redirect_uri(RedirectUrl::new(redirect_url.to_string()).unwrap());

        Self { client }
    }

    pub fn client(&self) -> &GitHubClient {
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

// ── Device Flow for desktop apps ──

/// Response from the device code request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Response from polling for the access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceTokenResponse {
    pub access_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Request a device code from GitHub for the Device Flow.
pub async fn request_device_code(client_id: &str) -> Result<DeviceCodeResponse, DeviceFlowError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(GITHUB_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "Artemis")
        .form(&[("client_id", client_id), ("scope", "gist read:user")])
        .send()
        .await
        .map_err(|e| DeviceFlowError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(DeviceFlowError::Api(body));
    }

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| DeviceFlowError::Parse(e.to_string()))
}

/// Poll GitHub for the access token (call this repeatedly with the interval).
pub async fn poll_device_token(
    client_id: &str,
    device_code: &str,
) -> Result<DeviceTokenResponse, DeviceFlowError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(GITHUB_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "Artemis")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| DeviceFlowError::Network(e.to_string()))?;

    resp.json::<DeviceTokenResponse>()
        .await
        .map_err(|e| DeviceFlowError::Parse(e.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceFlowError {
    #[error("network error: {0}")]
    Network(String),
    #[error("API error: {0}")]
    Api(String),
    #[error("parse error: {0}")]
    Parse(String),
}
