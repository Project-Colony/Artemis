use std::future::Future;
use std::pin::Pin;

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

/// A reqwest client that oauth2 can send its requests through.
///
/// oauth2's built-in reqwest support is tied to an older reqwest, so that
/// feature is off and this adapter takes its place.
pub struct OAuthHttp(pub reqwest::Client);

impl<'c> oauth2::AsyncHttpClient<'c> for OAuthHttp {
    type Error = oauth2::HttpClientError<reqwest::Error>;
    type Future =
        Pin<Box<dyn Future<Output = Result<oauth2::HttpResponse, Self::Error>> + Send + 'c>>;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let response = self
                .0
                .execute(request.try_into().map_err(Box::new)?)
                .await
                .map_err(Box::new)?;
            let mut builder = oauth2::http::Response::builder().status(response.status());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let body = response.bytes().await.map_err(Box::new)?;
            Ok(builder.body(body.to_vec())?)
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use oauth2::AsyncHttpClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn oauth_http_returns_status_headers_and_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 201 Created\r\nX-Test: yes\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
        });

        let request = oauth2::http::Request::post(format!("http://{addr}/token"))
            .body(b"code=abc".to_vec())
            .unwrap();
        let response = OAuthHttp(reqwest::Client::new())
            .call(request)
            .await
            .unwrap();

        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["x-test"], "yes");
        assert_eq!(response.body(), b"{}");
    }
}
