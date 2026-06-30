use base64::Engine;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct PoTokenManager {
    po_token: Arc<tokio::sync::Mutex<Option<String>>>,
    visitor_data: Arc<Mutex<String>>,
    po_token_endpoint: Option<String>,
    http_client: reqwest::Client,
}

impl PoTokenManager {
    pub fn new(potoken: Option<String>, po_token_endpoint: Option<String>, http_client: reqwest::Client) -> Self {
        Self {
            po_token: Arc::new(tokio::sync::Mutex::new(potoken)),
            visitor_data: Arc::new(Mutex::new(generate_visitor_data())),
            po_token_endpoint,
            http_client,
        }
    }

    pub fn visitor_data(&self) -> String {
        self.visitor_data.lock().unwrap().clone()
    }

    pub async fn get_token(&self) -> Option<String> {
        self.po_token.lock().await.clone()
    }

    pub async fn set_token(&self, token: String) {
        *self.po_token.lock().await = Some(token);
    }

    pub async fn try_generate(&self) -> Option<String> {
        if let Some(token) = self.get_token().await {
            return Some(token);
        }
        None
    }

    pub fn refresh_visitor_data(&self) {
        *self.visitor_data.lock().unwrap() = generate_visitor_data();
    }

    /// Spawns a background task that polls the PoToken endpoint periodically.
    /// The endpoint should return JSON like {"poToken": "...", "visitorData": "..."}.
    pub fn start_polling(&self) -> Option<tokio::task::JoinHandle<()>> {
        let endpoint = self.po_token_endpoint.clone()?;
        let po_token = self.po_token.clone();
        let visitor_data = self.visitor_data.clone();
        let http_client = self.http_client.clone();

        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                match http_client.get(&endpoint).send().await {
                    Ok(resp) => {
                        match resp.json::<serde_json::Value>().await {
                            Ok(json) => {
                                if let Some(token) = json.get("poToken").and_then(|v| v.as_str()) {
                                    *po_token.lock().await = Some(token.to_string());
                                }
                                if let Some(vd) = json.get("visitorData").and_then(|v| v.as_str()) {
                                    *visitor_data.lock().unwrap() = vd.to_string();
                                }
                            }
                            Err(e) => {
                                tracing::warn!(target: "PoToken", "Failed to parse PoToken response: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(target: "PoToken", "Failed to fetch PoToken from endpoint: {e}");
                    }
                }
            }
        }))
    }
}

impl Default for PoTokenManager {
    fn default() -> Self {
        Self::new(None, None, reqwest::Client::new())
    }
}

fn generate_visitor_data() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..24).map(|_| rng.gen()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
