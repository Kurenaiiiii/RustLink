use reqwest::Client;
use serde_json::Value;
use tracing::{info, warn};

const CLIENT_ID: &str = "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "SboVhoG9s0rNafixCSGGKXAT";
const SCOPES: &str = "http://gdata.youtube.com https://www.googleapis.com/auth/youtube";
const TOKEN_URL: &str = "https://www.youtube.com/o/oauth2/token";
const DEVICE_CODE_URL: &str = "https://www.youtube.com/o/oauth2/device/code";

#[derive(Clone)]
pub struct YouTubeOAuth {
    client: Client,
    refresh_tokens: Vec<String>,
    current_token_index: usize,
    access_token: Option<String>,
    token_expiry: tokio::time::Instant,
}

impl YouTubeOAuth {
    pub fn new(refresh_tokens: Vec<String>) -> Self {
        Self {
            client: Client::new(),
            refresh_tokens,
            current_token_index: 0,
            access_token: None,
            token_expiry: tokio::time::Instant::now(),
        }
    }

    pub fn has_tokens(&self) -> bool {
        !self.refresh_tokens.is_empty()
    }

    pub async fn get_access_token(&mut self) -> Option<String> {
        if !self.has_tokens() {
            return None;
        }

        if let Some(ref token) = self.access_token {
            if tokio::time::Instant::now() < self.token_expiry {
                return Some(token.clone());
            }
        }

        let max_attempts = self.refresh_tokens.len();
        let mut tried = 0;

        while tried < max_attempts {
            let current_token = match self.refresh_tokens.get(self.current_token_index) {
                Some(t) if !t.is_empty() => t.clone(),
                _ => {
                    self.current_token_index = (self.current_token_index + 1) % self.refresh_tokens.len();
                    tried += 1;
                    continue;
                }
            };

            for _ in 0..3 {
                match self.exchange_refresh_token(&current_token).await {
                    Ok(response) => {
                        self.access_token = Some(response.access_token.clone());
                        let expires_in = response.expires_in.max(60) - 30;
                        self.token_expiry = tokio::time::Instant::now() + tokio::time::Duration::from_secs(expires_in);
                        return Some(response.access_token);
                    }
                    Err(e) => {
                        warn!(target: "YouTubeOAuth", "Token refresh attempt failed: {e}");
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    }
                }
            }

            self.current_token_index = (self.current_token_index + 1) % self.refresh_tokens.len();
            tried += 1;
        }

        self.access_token = None;
        None
    }

    pub async fn get_auth_headers(&mut self) -> Vec<(String, String)> {
        match self.get_access_token().await {
            Some(token) => vec![("Authorization".into(), format!("Bearer {token}"))],
            None => vec![],
        }
    }

    async fn exchange_refresh_token(&self, refresh_token: &str) -> anyhow::Result<OAuthTokenResponse> {
        let params = [
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let resp = self
            .client
            .post(TOKEN_URL)
            .form(&params)
            .send()
            .await?
            .json::<Value>()
            .await?;

        let access_token = resp["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing access_token in OAuth response"))?
            .to_string();

        let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);

        Ok(OAuthTokenResponse {
            access_token,
            expires_in,
        })
    }

    pub async fn acquire_refresh_token() -> anyhow::Result<String> {
        let client = Client::new();

        let params = [("client_id", CLIENT_ID), ("scope", SCOPES)];
        let device_resp = client
            .post(DEVICE_CODE_URL)
            .form(&params)
            .send()
            .await?
            .json::<Value>()
            .await?;

        let device_code = device_resp["device_code"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing device_code"))?
            .to_string();
        let user_code = device_resp["user_code"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing user_code"))?
            .to_string();
        let verification_url = device_resp["verification_url"]
            .as_str()
            .unwrap_or("https://www.google.com/device");
        let interval = device_resp["interval"].as_u64().unwrap_or(5);

        info!(target: "YouTubeOAuth", "==================================================================");
        info!(target: "YouTubeOAuth", "ALERT: DO NOT USE YOUR MAIN GOOGLE ACCOUNT! USE A SECONDARY ACCOUNT ONLY!");
        info!(target: "YouTubeOAuth", "To authorize, visit the following URL in your browser:");
        info!(target: "YouTubeOAuth", "URL: {verification_url}");
        info!(target: "YouTubeOAuth", "And enter the code: {user_code}");
        info!(target: "YouTubeOAuth", "==================================================================");

        Self::poll_for_token(&client, &device_code, interval).await
    }

    async fn poll_for_token(client: &Client, device_code: &str, interval: u64) -> anyhow::Result<String> {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;

            let params = [
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("code", device_code),
                ("grant_type", "http://oauth.net/grant_type/device/1.0"),
            ];

            let resp = client
                .post(TOKEN_URL)
                .form(&params)
                .send()
                .await?
                .json::<Value>()
                .await?;

            if let Some(error) = resp["error"].as_str() {
                match error {
                    "authorization_pending" => continue,
                    "slow_down" => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                    "expired_token" => anyhow::bail!("Authorization code expired"),
                    "access_denied" => anyhow::bail!("Access denied"),
                    _ => anyhow::bail!("OAuth error: {}", error),
                }
            }

            let refresh_token = resp["refresh_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing refresh_token in OAuth response"))?
                .to_string();

            info!(target: "YouTubeOAuth", "==================================================================");
            info!(target: "YouTubeOAuth", "Authorization granted successfully!");
            info!(target: "YouTubeOAuth", "Copy your Refresh Token and add it to your config:");
            info!(target: "YouTubeOAuth", "{refresh_token}");
            info!(target: "YouTubeOAuth", "==================================================================");

            return Ok(refresh_token);
        }
    }
}

struct OAuthTokenResponse {
    access_token: String,
    expires_in: u64,
}
