use chrono::Utc;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::crypto::{Identity, Purpose};
use crate::protocol::{ConnectionInfo, ProfileGist, SignalingMessage};

const GITHUB_API: &str = "https://api.github.com";

/// GitHub Gist-based signaling layer for peer discovery and connection.
pub struct GistSignaling {
    token: String,
    client: reqwest::Client,
    username: String,
    /// Our profile Gist ID (public, contains public key + signaling Gist ID).
    profile_gist_id: Option<String>,
    /// Our signaling/mailbox Gist ID (secret, peers post connection requests here).
    signaling_gist_id: Option<String>,
}

/// A Gist returned by the GitHub API.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GistResponse {
    id: String,
    description: Option<String>,
    public: bool,
    files: std::collections::HashMap<String, GistFile>,
    comments: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GistFile {
    filename: String,
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateGistRequest {
    description: String,
    public: bool,
    files: std::collections::HashMap<String, GistFileContent>,
}

#[derive(Debug, Serialize)]
struct GistFileContent {
    content: String,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct UpdateGistRequest {
    files: std::collections::HashMap<String, GistFileContent>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GistComment {
    id: u64,
    body: String,
    user: GistCommentUser,
    created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GistCommentUser {
    login: String,
}

impl GistComment {
    /// Parse the comment as a signaling message, keeping it only if the sender
    /// it names is the GitHub account that posted it. Anyone can comment on a
    /// mailbox Gist, so `from_username` in the body proves nothing on its own.
    fn message(&self) -> Option<SignalingMessage> {
        let msg: SignalingMessage = serde_json::from_str(&self.body).ok()?;
        let claimed = match &msg {
            SignalingMessage::ConnectRequest { from_username, .. }
            | SignalingMessage::ConnectAccept { from_username, .. }
            | SignalingMessage::FriendRequest { from_username, .. }
            | SignalingMessage::FriendAccepted { from_username, .. } => from_username,
        };
        claimed
            .eq_ignore_ascii_case(&self.user.login)
            .then_some(msg)
    }
}

#[derive(Debug, Serialize)]
struct CreateCommentRequest {
    body: String,
}

impl GistSignaling {
    /// Create a new signaling client with a GitHub access token.
    ///
    /// The HTTP client refuses every URL that is not HTTPS, redirects
    /// included, so the token and peer names never travel in cleartext.
    pub fn new(token: String, username: String) -> Result<Self, SignalingError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .build()
            .map_err(|e| SignalingError::Network(e.to_string()))?;
        Ok(Self {
            token,
            client,
            username,
            profile_gist_id: None,
            signaling_gist_id: None,
        })
    }

    /// Initialize the signaling layer: find or create profile + signaling Gists.
    pub async fn initialize(&mut self, identity: &Identity) -> Result<(), SignalingError> {
        // Try to find existing profile Gist
        let existing = self.find_own_profile_gist().await?;

        if let Some((profile_gist_id, profile)) = existing {
            self.profile_gist_id = Some(profile_gist_id);
            self.signaling_gist_id = Some(profile.signaling_gist_id);
            info!(
                "Found existing profile Gist, signaling Gist: {}",
                self.signaling_gist_id.as_deref().unwrap_or("?")
            );
        } else {
            // Create signaling (mailbox) Gist first
            let signaling_id = self.create_signaling_gist().await?;
            self.signaling_gist_id = Some(signaling_id.clone());

            // Create public profile Gist
            let profile = ProfileGist {
                public_key: identity.public_key_b64(),
                signaling_gist_id: signaling_id,
                version: "0.1.0".to_string(),
            };
            let profile_id = self.create_profile_gist(&profile).await?;
            self.profile_gist_id = Some(profile_id);

            info!("Created profile and signaling Gists");
        }

        Ok(())
    }

    /// Look up a peer's profile by their GitHub username.
    pub async fn lookup_peer(
        &self,
        github_username: &str,
    ) -> Result<Option<ProfileGist>, SignalingError> {
        // Code scanning reports this as cleartext transmission because a
        // username reaches a request URL. `api_url` always builds on GITHUB_API
        // (https://api.github.com) and the client is `https_only`, so the name
        // is only ever sent over TLS.
        let url = api_url(&["users", github_username, "gists"])?;
        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let gists: Vec<GistResponse> = resp
            .json()
            .await
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

        // Find the Gist with an `artemis-profile.json` file
        for gist in &gists {
            if gist.files.contains_key("artemis-profile.json") {
                // Fetch the full Gist content
                let full = self.get_gist(&gist.id).await?;
                if let Some(file) = full.files.get("artemis-profile.json") {
                    if let Some(content) = &file.content {
                        let profile: ProfileGist = serde_json::from_str(content)
                            .map_err(|e| SignalingError::Parse(e.to_string()))?;
                        return Ok(Some(profile));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Post a connection request to a peer's signaling Gist (as a comment).
    pub async fn send_connect_request(
        &self,
        peer_signaling_gist_id: &str,
        identity: &Identity,
        peer_public_key: &str,
        conn_info: &ConnectionInfo,
    ) -> Result<(), SignalingError> {
        let info_json =
            serde_json::to_string(conn_info).map_err(|e| SignalingError::Parse(e.to_string()))?;

        let encrypted = identity
            .encrypt_for_b64(Purpose::Signaling, peer_public_key, info_json.as_bytes())
            .map_err(|e| SignalingError::Crypto(e.to_string()))?;

        let msg = SignalingMessage::ConnectRequest {
            from_username: self.username.clone(),
            encrypted_info: encrypted,
            timestamp: Utc::now(),
        };

        let body = serde_json::to_string(&msg).map_err(|e| SignalingError::Parse(e.to_string()))?;

        self.post_comment(peer_signaling_gist_id, &body).await?;
        info!("Sent connection request to peer's signaling Gist");
        Ok(())
    }

    /// Poll our signaling Gist for new connection requests.
    pub async fn poll_requests(
        &self,
        identity: &Identity,
    ) -> Result<Vec<(String, ConnectionInfo)>, SignalingError> {
        let gist_id = self
            .signaling_gist_id
            .as_deref()
            .ok_or(SignalingError::NotInitialized)?;

        let comments = self.get_comments(gist_id).await?;
        let mut results = Vec::new();

        for comment in comments {
            match comment.message() {
                Some(SignalingMessage::ConnectRequest {
                    from_username,
                    encrypted_info,
                    ..
                }) => {
                    // We need the sender's public key to decrypt.
                    // Look up their profile first.
                    if let Ok(Some(profile)) = self.lookup_peer(&from_username).await {
                        match identity.decrypt_from_b64(
                            Purpose::Signaling,
                            &profile.public_key,
                            &encrypted_info,
                        ) {
                            Ok(plaintext) => {
                                if let Ok(conn_info) =
                                    serde_json::from_slice::<ConnectionInfo>(&plaintext)
                                {
                                    results.push((from_username, conn_info));
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to decrypt connection request from {}: {}",
                                    from_username, e
                                );
                            }
                        }
                    }
                }
                Some(SignalingMessage::FriendRequest {
                    from_username,
                    display_name,
                    ..
                }) => {
                    info!(
                        "Friend request from {} ({})",
                        from_username,
                        display_name.as_deref().unwrap_or("?")
                    );
                    // Friend requests are handled separately
                }
                _ => {
                    // Ignore other message types or parse errors
                }
            }
        }

        Ok(results)
    }

    /// Send a friend request to a peer.
    pub async fn send_friend_request(
        &self,
        peer_signaling_gist_id: &str,
        identity: &Identity,
        display_name: Option<String>,
        avatar_url: Option<String>,
    ) -> Result<(), SignalingError> {
        let msg = SignalingMessage::FriendRequest {
            from_username: self.username.clone(),
            public_key: identity.public_key_b64(),
            display_name,
            avatar_url,
            timestamp: Utc::now(),
        };

        let body = serde_json::to_string(&msg).map_err(|e| SignalingError::Parse(e.to_string()))?;

        self.post_comment(peer_signaling_gist_id, &body).await?;
        Ok(())
    }

    /// Poll for incoming friend requests.
    pub async fn poll_friend_requests(&self) -> Result<Vec<SignalingMessage>, SignalingError> {
        let gist_id = self
            .signaling_gist_id
            .as_deref()
            .ok_or(SignalingError::NotInitialized)?;

        let comments = self.get_comments(gist_id).await?;
        let mut results = Vec::new();

        for comment in comments {
            if let Some(msg @ SignalingMessage::FriendRequest { .. }) = comment.message() {
                results.push(msg);
            }
        }

        Ok(results)
    }

    /// Get our signaling Gist ID.
    pub fn signaling_gist_id(&self) -> Option<&str> {
        self.signaling_gist_id.as_deref()
    }

    /// Get our profile Gist ID.
    pub fn profile_gist_id(&self) -> Option<&str> {
        self.profile_gist_id.as_deref()
    }

    // ── Private helpers ──

    async fn find_own_profile_gist(&self) -> Result<Option<(String, ProfileGist)>, SignalingError> {
        let url = format!("{}/gists", GITHUB_API);
        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .query(&[("per_page", "100")])
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        let gists: Vec<GistResponse> = resp
            .json()
            .await
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

        for gist in &gists {
            if gist.files.contains_key("artemis-profile.json") {
                let full = self.get_gist(&gist.id).await?;
                if let Some(file) = full.files.get("artemis-profile.json") {
                    if let Some(content) = &file.content {
                        if let Ok(profile) = serde_json::from_str::<ProfileGist>(content) {
                            return Ok(Some((gist.id.clone(), profile)));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    async fn create_signaling_gist(&self) -> Result<String, SignalingError> {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "artemis-signaling.json".to_string(),
            GistFileContent {
                content: serde_json::json!({
                    "description": "Artemis P2P signaling mailbox — do not delete",
                    "version": "0.1.0"
                })
                .to_string(),
            },
        );

        let req = CreateGistRequest {
            description: "Artemis signaling mailbox".to_string(),
            public: false,
            files,
        };

        let resp = self
            .client
            .post(format!("{}/gists", GITHUB_API))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .json(&req)
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        let gist: GistResponse = resp
            .json()
            .await
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

        Ok(gist.id)
    }

    async fn create_profile_gist(&self, profile: &ProfileGist) -> Result<String, SignalingError> {
        let mut files = std::collections::HashMap::new();
        files.insert(
            "artemis-profile.json".to_string(),
            GistFileContent {
                content: serde_json::to_string_pretty(profile)
                    .map_err(|e| SignalingError::Parse(e.to_string()))?,
            },
        );

        let req = CreateGistRequest {
            description: "Artemis P2P profile — public key + signaling".to_string(),
            public: true,
            files,
        };

        let resp = self
            .client
            .post(format!("{}/gists", GITHUB_API))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .json(&req)
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        let gist: GistResponse = resp
            .json()
            .await
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

        Ok(gist.id)
    }

    async fn get_gist(&self, gist_id: &str) -> Result<GistResponse, SignalingError> {
        let resp = self
            .client
            .get(api_url(&["gists", gist_id])?)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| SignalingError::Parse(e.to_string()))
    }

    async fn get_comments(&self, gist_id: &str) -> Result<Vec<GistComment>, SignalingError> {
        let resp = self
            .client
            .get(api_url(&["gists", gist_id, "comments"])?)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| SignalingError::Parse(e.to_string()))
    }

    async fn post_comment(&self, gist_id: &str, body: &str) -> Result<(), SignalingError> {
        let req = CreateCommentRequest {
            body: body.to_string(),
        };

        self.client
            .post(api_url(&["gists", gist_id, "comments"])?)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "Artemis")
            .header(ACCEPT, "application/vnd.github+json")
            .json(&req)
            .send()
            .await
            .map_err(|e| SignalingError::Network(e.to_string()))?;

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("network error: {0}")]
    Network(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("signaling not initialized — call initialize() first")]
    NotInitialized,
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("not a valid GitHub user name or Gist ID")]
    InvalidId,
}

/// Build a GitHub API URL from path segments.
///
/// User names and Gist IDs come from comments and profiles anyone can write,
/// and every request carries our token. A segment is therefore limited to
/// what GitHub logins and Gist IDs use (ASCII letters, digits, hyphens), so
/// no `/`, `..`, `%`, `?` or `#` can point the request at another endpoint.
fn api_url(segments: &[&str]) -> Result<String, SignalingError> {
    let plain = |s: &&str| {
        (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !segments.iter().all(plain) {
        return Err(SignalingError::InvalidId);
    }
    Ok(format!("{GITHUB_API}/{}", segments.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_urls_only_take_plain_names_and_ids() {
        assert_eq!(
            api_url(&["users", "Mother-Sphere42", "gists"]).unwrap(),
            "https://api.github.com/users/Mother-Sphere42/gists"
        );
        let too_long = "a".repeat(65);
        for bad in [
            "", "../user", "a/b", "a?x=1", "a#b", "a.b", "%2e%2e", &too_long,
        ] {
            assert!(api_url(&["gists", bad]).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn comments_must_come_from_the_claimed_sender() {
        let comment = |login: &str| GistComment {
            id: 1,
            body: r#"{"type":"FriendRequest","from_username":"alice","public_key":"",
                "display_name":null,"avatar_url":null,"timestamp":"2026-01-01T00:00:00Z"}"#
                .to_string(),
            user: GistCommentUser {
                login: login.to_string(),
            },
            created_at: String::new(),
        };
        assert!(comment("Alice").message().is_some());
        assert!(comment("mallory").message().is_none());
    }

    #[tokio::test]
    async fn client_refuses_plain_http() {
        let signaling = GistSignaling::new(String::new(), "me".to_string()).unwrap();
        // `https_only` rejects the URL before any connection is attempted.
        let err = signaling
            .client
            .get("http://127.0.0.1:9/")
            .send()
            .await
            .unwrap_err();
        assert!(err.is_builder(), "{err}");
    }
}
