use chrono::Utc;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::crypto::Identity;
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
struct UpdateGistRequest {
    files: std::collections::HashMap<String, GistFileContent>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Serialize)]
struct CreateCommentRequest {
    body: String,
}

impl GistSignaling {
    /// Create a new signaling client with a GitHub access token.
    pub fn new(token: String, username: String) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
            username,
            profile_gist_id: None,
            signaling_gist_id: None,
        }
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
    pub async fn lookup_peer(&self, github_username: &str) -> Result<Option<ProfileGist>, SignalingError> {
        let url = format!("{}/users/{}/gists", GITHUB_API, github_username);
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
        let info_json = serde_json::to_string(conn_info)
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

        let encrypted = identity
            .encrypt_for_b64(peer_public_key, info_json.as_bytes())
            .map_err(|e| SignalingError::Crypto(e.to_string()))?;

        let msg = SignalingMessage::ConnectRequest {
            from_username: self.username.clone(),
            encrypted_info: encrypted,
            timestamp: Utc::now(),
        };

        let body = serde_json::to_string(&msg)
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

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
            match serde_json::from_str::<SignalingMessage>(&comment.body) {
                Ok(SignalingMessage::ConnectRequest {
                    from_username,
                    encrypted_info,
                    ..
                }) => {
                    // We need the sender's public key to decrypt.
                    // Look up their profile first.
                    if let Ok(Some(profile)) = self.lookup_peer(&from_username).await {
                        match identity.decrypt_from_b64(&profile.public_key, &encrypted_info) {
                            Ok(plaintext) => {
                                if let Ok(conn_info) =
                                    serde_json::from_slice::<ConnectionInfo>(&plaintext)
                                {
                                    results.push((from_username, conn_info));
                                }
                            }
                            Err(e) => {
                                warn!("Failed to decrypt connection request from {}: {}", from_username, e);
                            }
                        }
                    }
                }
                Ok(SignalingMessage::FriendRequest {
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

        let body = serde_json::to_string(&msg)
            .map_err(|e| SignalingError::Parse(e.to_string()))?;

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
            if let Ok(msg @ SignalingMessage::FriendRequest { .. }) =
                serde_json::from_str(&comment.body)
            {
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
            .post(&format!("{}/gists", GITHUB_API))
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
            .post(&format!("{}/gists", GITHUB_API))
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
            .get(&format!("{}/gists/{}", GITHUB_API, gist_id))
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
            .get(&format!("{}/gists/{}/comments", GITHUB_API, gist_id))
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
            .post(&format!("{}/gists/{}/comments", GITHUB_API, gist_id))
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
}
