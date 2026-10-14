use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use artemis_core::protocol::DeviceCode;

/// GitHub user profile returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Why a device flow request gave no answer to act on yet.
#[derive(Debug, thiserror::Error)]
pub enum DeviceFlowError {
    #[error("the code has not been entered yet")]
    Pending,
    /// Poll every `interval` seconds from now on. GitHub always sends it; when
    /// it does not, add 5 seconds to the current interval.
    #[error("polling too fast")]
    SlowDown { interval: Option<u64> },
    #[error("access denied by the user")]
    Denied,
    #[error("the code expired")]
    Expired,
    /// Any other answer, such as an unknown device code or an OAuth app
    /// without the device flow enabled.
    #[error("GitHub refused the request: {0}")]
    Refused(String),
    #[error("cannot reach GitHub: {0}")]
    Http(#[from] reqwest::Error),
}

/// An OAuth app's device flow on GitHub, plus the one API call it needs.
#[derive(Clone)]
pub struct GitHub {
    http: reqwest::Client,
    client_id: String,
    /// `https://github.com`, which serves the OAuth endpoints.
    web_url: String,
    /// `https://api.github.com`.
    api_url: String,
}

impl GitHub {
    pub fn new(client_id: &str) -> reqwest::Result<Self> {
        Self::with_urls(client_id, "https://github.com", "https://api.github.com")
    }

    /// Talks to other hosts than GitHub's, such as a stub in tests.
    pub fn with_urls(client_id: &str, web_url: &str, api_url: &str) -> reqwest::Result<Self> {
        let http = reqwest::Client::builder()
            // A redirect could send the device code or the token elsewhere.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent("Artemis")
            .build()?;
        Ok(Self {
            http,
            client_id: client_id.to_string(),
            web_url: web_url.to_string(),
            api_url: api_url.to_string(),
        })
    }

    /// Asks for a code the user enters on GitHub to grant `read:user`.
    /// GitHub's answer is passed on to clients as it is.
    pub async fn request_device_code(&self) -> Result<DeviceCode, DeviceFlowError> {
        self.post(
            "/login/device/code",
            &[("client_id", &self.client_id), ("scope", "read:user")],
        )
        .await
    }

    /// Asks GitHub once whether the user has entered the code, and returns
    /// the access token once they have.
    pub async fn poll_device_token(&self, device_code: &str) -> Result<String, DeviceFlowError> {
        #[derive(Deserialize)]
        struct Token {
            access_token: String,
        }
        let token: Token = self
            .post(
                "/login/oauth/access_token",
                &[
                    ("client_id", &self.client_id),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ],
            )
            .await?;
        Ok(token.access_token)
    }

    /// The profile of the user `access_token` belongs to.
    pub async fn fetch_user(&self, access_token: &str) -> Result<GitHubUser, DeviceFlowError> {
        Ok(self
            .http
            .get(format!("{}/user", self.api_url))
            .header(ACCEPT, "application/vnd.github+json")
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// POSTs a form to an OAuth endpoint. GitHub answers an OAuth error with
    /// `{"error": "..."}` in place of the expected fields.
    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<T, DeviceFlowError> {
        let response = self
            .http
            .post(format!("{}{path}", self.web_url))
            .header(ACCEPT, "application/json")
            .form(form)
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        if let Ok(answer) = serde_json::from_slice(&body) {
            return Ok(answer);
        }

        #[derive(Deserialize)]
        struct OAuthError {
            error: String,
            interval: Option<u64>,
        }
        let Ok(e) = serde_json::from_slice::<OAuthError>(&body) else {
            return Err(DeviceFlowError::Refused(format!("HTTP {status}")));
        };
        Err(match e.error.as_str() {
            "authorization_pending" => DeviceFlowError::Pending,
            "slow_down" => DeviceFlowError::SlowDown {
                interval: e.interval,
            },
            "access_denied" => DeviceFlowError::Denied,
            "expired_token" => DeviceFlowError::Expired,
            _ => DeviceFlowError::Refused(e.error),
        })
    }
}
