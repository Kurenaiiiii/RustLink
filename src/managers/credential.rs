use std::sync::Arc;
use std::time::{Duration, Instant};
use dashmap::DashMap;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct OAuth2Config {
    pub client_id: String,
    pub client_secret: String,
    pub token_url: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TokenEntry {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Instant,
    pub token_type: String,
}

#[derive(Debug, Clone)]
pub struct StaticCredential {
    pub value: String,
    pub header_name: String,
}

#[derive(Debug, Clone)]
pub enum CredentialValue {
    OAuth2(OAuth2Config),
    BearerToken(Arc<RwLock<Option<TokenEntry>>>),
    Static(StaticCredential),
    Raw(String),
}

#[derive(Clone)]
pub struct CredentialStore {
    inner: Arc<DashMap<String, CredentialValue>>,
}

impl CredentialStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    pub fn set(&self, provider: &str, value: CredentialValue) {
        self.inner.insert(provider.to_string(), value);
    }

    pub fn get(&self, provider: &str) -> Option<CredentialValue> {
        self.inner.get(provider).map(|r| r.value().clone())
    }

    pub fn set_static(&self, provider: &str, value: &str, header: &str) {
        self.set(
            provider,
            CredentialValue::Static(StaticCredential {
                value: value.to_string(),
                header_name: header.to_string(),
            }),
        );
    }

    pub fn set_raw(&self, provider: &str, value: &str) {
        self.set(provider, CredentialValue::Raw(value.to_string()));
    }

    pub fn set_oauth2(&self, provider: &str, config: OAuth2Config) {
        self.set(provider, CredentialValue::OAuth2(config));
    }
}

pub struct CredentialManager {
    store: CredentialStore,
}

impl CredentialManager {
    pub fn new(store: CredentialStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &CredentialStore {
        &self.store
    }

    pub async fn get_access_token(&self, provider: &str) -> Option<String> {
        let entry = self.store.get(provider)?;
        match entry {
            CredentialValue::BearerToken(token_rw) => {
                let token = token_rw.read().await;
                token.as_ref().map(|t| t.access_token.clone())
            }
            CredentialValue::Static(s) => Some(s.value.clone()),
            CredentialValue::Raw(s) => Some(s),
            CredentialValue::OAuth2(config) => {
                self.refresh_oauth2(provider, &config).await
            }
        }
    }

    pub async fn ensure_token(&self, provider: &str) -> Option<String> {
        let entry = self.store.get(provider)?;
        match entry {
            CredentialValue::BearerToken(token_rw) => {
                let needs_refresh = {
                    let token = token_rw.read().await;
                    token.as_ref().map(|t| t.expires_at <= Instant::now()).unwrap_or(true)
                };
                if needs_refresh {
                    None
                } else {
                    let token = token_rw.read().await;
                    token.as_ref().map(|t| t.access_token.clone())
                }
            }
            CredentialValue::OAuth2(config) => {
                self.refresh_oauth2(provider, &config).await
            }
            CredentialValue::Static(s) => Some(s.value.clone()),
            CredentialValue::Raw(s) => Some(s),
        }
    }

    async fn refresh_oauth2(
        &self,
        provider: &str,
        config: &OAuth2Config,
    ) -> Option<String> {
        let client = reqwest::Client::new();
        let mut params: Vec<(&str, &str)> = Vec::new();
        params.push(("grant_type", "client_credentials"));
        params.push(("client_id", &config.client_id));
        params.push(("client_secret", &config.client_secret));

        let scope;
        if !config.scopes.is_empty() {
            scope = config.scopes.join(" ");
            params.push(("scope", &scope));
        }

        let resp = client
            .post(&config.token_url)
            .form(&params)
            .send()
            .await
            .ok()?;

        let body: serde_json::Value = resp.json().await.ok()?;
        let access_token = body["access_token"].as_str()?.to_string();
        let expires_in = body["expires_in"].as_u64().unwrap_or(3600);

        let entry = TokenEntry {
            access_token: access_token.clone(),
            refresh_token: body["refresh_token"].as_str().map(String::from),
            expires_at: Instant::now() + Duration::from_secs(expires_in.saturating_sub(60)),
            token_type: body["token_type"].as_str().unwrap_or("Bearer").to_string(),
        };

        self.store.set(
            provider,
            CredentialValue::BearerToken(Arc::new(RwLock::new(Some(entry)))),
        );

        Some(access_token)
    }
}
