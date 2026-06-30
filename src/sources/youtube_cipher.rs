use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};
use serde_json::Value;

const CACHE_DURATION_MS: u64 = 12 * 60 * 60 * 1000;

#[derive(Clone)]
pub struct CipherManager {
    client: Client,
    /// Cached player script URL from watch page discovery
    player_url: Arc<Mutex<Option<String>>>,
    player_expiry: Arc<Mutex<tokio::time::Instant>>,
    /// Explicitly configured player script URL (overrides discovery)
    explicit_player_url: Arc<Mutex<Option<ExplicitScript>>>,
    /// STS cache: player_url -> (sts, expiry)
    sts_cache: Arc<Mutex<HashMap<String, (String, tokio::time::Instant)>>>,
    /// Decipher operations cache
    ops_cache: Arc<Mutex<Option<(Vec<DecipherOp>, tokio::time::Instant)>>>,
    /// Cooperative lock for concurrent discovery
    loading: Arc<Mutex<bool>>,
    /// Remote cipher service configuration
    remote_url: Option<String>,
    remote_token: Option<String>,
    user_agent: String,
}

#[derive(Debug, Clone)]
struct ExplicitScript {
    url: String,
    expiry: tokio::time::Instant,
}

#[derive(Debug, Clone)]
pub enum DecipherOp {
    Reverse,
    Splice(usize),
    Swap(usize),
}

impl CipherManager {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .unwrap(),
            player_url: Arc::new(Mutex::new(None)),
            player_expiry: Arc::new(Mutex::new(tokio::time::Instant::now())),
            explicit_player_url: Arc::new(Mutex::new(None)),
            sts_cache: Arc::new(Mutex::new(HashMap::new())),
            ops_cache: Arc::new(Mutex::new(None)),
            loading: Arc::new(Mutex::new(false)),
            remote_url: None,
            remote_token: None,
            user_agent: format!("rustlink/{} (https://github.com/anomalyco/RustRewrite)", "3.8.0"),
        }
    }

    pub fn configure_remote(mut self, url: Option<String>, token: Option<String>) -> Self {
        self.remote_url = url.map(|u| u.trim_end_matches('/').to_string());
        self.remote_token = token;
        self
    }

    pub fn cleanup(&self) {
        // No timers to clean in Rust (no setInterval equivalent needed)
    }

    pub async fn set_player_script_url(&self, url: String) {
        let full_url = if url.starts_with("http") { url } else { format!("https://www.youtube.com{url}") };
        let mut explicit = self.explicit_player_url.lock().await;
        *explicit = Some(ExplicitScript {
            url: full_url,
            expiry: tokio::time::Instant::now() + tokio::time::Duration::from_millis(CACHE_DURATION_MS),
        });
        info!(target: "YouTube-Cipher", "Explicit player script URL set");
    }

    pub async fn get_player_script(&self) -> Option<String> {
        if self.cipher_load_lock().await {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            return self.get_cached_player_script().await;
        }

        // Check explicit URL first
        {
            let explicit = self.explicit_player_url.lock().await;
            if let Some(es) = explicit.as_ref() {
                if tokio::time::Instant::now() < es.expiry {
                    *self.player_url.lock().await = Some(es.url.clone());
                    *self.player_expiry.lock().await = es.expiry;
                    return Some(es.url.clone());
                }
            }
        }

        let mut loading = self.loading.lock().await;
        if *loading {
            drop(loading);
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            return self.player_url.lock().await.clone();
        }
        *loading = true;
        drop(loading);

        let url = self.fetch_player_url_from_watch_page().await;

        let mut loading = self.loading.lock().await;
        *loading = false;

        url
    }

    async fn cipher_load_lock(&self) -> bool {
        // Simple spin-lock check
        false
    }

    pub async fn get_cached_player_script(&self) -> Option<String> {
        // Check explicit
        {
            let explicit = self.explicit_player_url.lock().await;
            if let Some(es) = explicit.as_ref() {
                if tokio::time::Instant::now() < es.expiry {
                    return Some(es.url.clone());
                }
            }
        }
        // Check cached
        {
            let expiry = self.player_expiry.lock().await;
            let url = self.player_url.lock().await;
            if tokio::time::Instant::now() < *expiry {
                return url.clone();
            }
        }
        // Fetch fresh — call the internal impl directly to avoid recursion
        if self.cipher_load_lock().await {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            return self.player_url.lock().await.clone();
        }

        let mut loading = self.loading.lock().await;
        if *loading {
            drop(loading);
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            return self.player_url.lock().await.clone();
        }
        *loading = true;
        drop(loading);

        // Check explicit URL first
        {
            let explicit = self.explicit_player_url.lock().await;
            if let Some(es) = explicit.as_ref() {
                if tokio::time::Instant::now() < es.expiry {
                    *self.player_url.lock().await = Some(es.url.clone());
                    *self.player_expiry.lock().await = es.expiry;
                    let mut loading = self.loading.lock().await;
                    *loading = false;
                    return Some(es.url.clone());
                }
            }
        }

        let url = self.fetch_player_url_from_watch_page().await;
        let mut loading = self.loading.lock().await;
        *loading = false;
        url
    }

    async fn fetch_player_url_from_watch_page(&self) -> Option<String> {
        let watch_url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let resp = self
            .client
            .get(watch_url)
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .send()
            .await
            .ok()?;

        let body = resp.text().await.ok()?;

        let start = body.find("\"jsUrl\":\"")?;
        let start = start + "\"jsUrl\":\"".len();
        let end = body[start..].find('"')?;
        let relative_url = &body[start..start + end];

        let fixed_url = if let Some(pos) = relative_url.rfind('/') {
            let base_path = &relative_url[..pos];
            format!("{base_path}/en_US/base.js")
        } else {
            relative_url.to_string()
        };
        let full_url = format!("https://www.youtube.com{fixed_url}");

        *self.player_url.lock().await = Some(full_url.clone());
        *self.player_expiry.lock().await =
            tokio::time::Instant::now() + tokio::time::Duration::from_millis(CACHE_DURATION_MS);

        info!(target: "YouTube-Cipher", "Player script URL: {full_url}");
        Some(full_url)
    }

    pub async fn get_signature_timestamp(&self) -> Option<String> {
        let player_url = self.get_cached_player_script().await?;

        {
            let cache = self.sts_cache.lock().await;
            if let Some((sts, expiry)) = cache.get(&player_url) {
                if tokio::time::Instant::now() < *expiry {
                    return Some(sts.clone());
                }
            }
        }

        // Try remote cipher service first
        if let Some(ref remote) = self.remote_url {
            if let Some(sts) = self.fetch_sts_from_remote(remote, &player_url).await {
                return Some(sts);
            }
        }

        // Fall back to local extraction
        let resp = self.client.get(&player_url).send().await.ok()?;
        let body = resp.text().await.ok()?;

        let sts = self.extract_sts_from_script(&body)?;

        let mut cache = self.sts_cache.lock().await;
        cache.insert(
            player_url,
            (
                sts.clone(),
                tokio::time::Instant::now() + tokio::time::Duration::from_millis(CACHE_DURATION_MS),
            ),
        );
        Some(sts)
    }

    async fn fetch_sts_from_remote(&self, remote: &str, player_url: &str) -> Option<String> {
        let mut req = self
            .client
            .post(format!("{remote}/get_sts"))
            .json(&serde_json::json!({ "player_url": player_url }))
            .header("Content-Type", "application/json")
            .header("User-Agent", &self.user_agent);

        if let Some(ref token) = self.remote_token {
            req = req.header("Authorization", token);
        }

        let resp = req.send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let data: Value = resp.json().await.ok()?;
        data["sts"].as_str().map(|s| s.to_string())
    }

    fn extract_sts_from_script(&self, body: &str) -> Option<String> {
        for pattern in &["signatureTimestamp\":", "sts\":", "signatureTimestamp=", "sts="] {
            if let Some(idx) = body.find(pattern) {
                let start = idx + pattern.len();
                let end = start + body[start..].find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
                let sts = body[start..end].to_string();
                if !sts.is_empty() {
                    return Some(sts);
                }
            }
        }
        None
    }

    pub async fn check_cipher_server_status(&self) -> bool {
        let remote = match self.remote_url.as_ref() {
            Some(r) => r,
            None => return false,
        };
        let mut req = self.client.get(format!("{remote}/"))
            .header("User-Agent", &self.user_agent);
        if let Some(ref token) = self.remote_token {
            req = req.header("Authorization", token);
        }
        match req.send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    pub async fn get_decipher_ops(&self) -> Option<Vec<DecipherOp>> {
        {
            let cache = self.ops_cache.lock().await;
            if let Some((ops, expiry)) = cache.as_ref() {
                if tokio::time::Instant::now() < *expiry {
                    return Some(ops.clone());
                }
            }
        }

        let player_url = self.get_cached_player_script().await?;
        let resp = self.client.get(&player_url).send().await.ok()?;
        let body = resp.text().await.ok()?;

        let ops = self.extract_decipher_ops(&body)?;

        let mut cache = self.ops_cache.lock().await;
        *cache = Some((
            ops.clone(),
            tokio::time::Instant::now() + tokio::time::Duration::from_millis(CACHE_DURATION_MS),
        ));
        Some(ops)
    }

    pub async fn resolve_url(&self, stream_url: &str, encrypted_sig: Option<&str>, n_param: Option<&str>, sig_key: Option<&str>) -> Result<String, String> {
        let remote = self.remote_url.as_ref().ok_or_else(|| "Remote cipher URL is not configured".to_string())?;
        let player_url = self.get_cached_player_script().await.ok_or_else(|| "No player script available".to_string())?;

        let mut body = serde_json::json!({
            "stream_url": stream_url,
            "player_url": player_url,
        });
        if let Some(sig) = encrypted_sig {
            body["encrypted_signature"] = serde_json::Value::String(sig.to_string());
            body["signature_key"] = serde_json::Value::String(sig_key.unwrap_or("sig").to_string());
        }
        if let Some(n) = n_param {
            body["n_param"] = serde_json::Value::String(n.to_string());
        }

        let mut req = self.client.post(format!("{remote}/resolve_url"))
            .json(&body)
            .header("Content-Type", "application/json")
            .header("User-Agent", &self.user_agent);
        if let Some(ref token) = self.remote_token {
            req = req.header("Authorization", token);
        }

        let resp = req.send().await.map_err(|e| format!("Request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("Cipher service returned {}", resp.status()));
        }
        let data: Value = resp.json().await.map_err(|e| format!("Parse failed: {e}"))?;
        data["resolved_url"].as_str().map(|s| s.to_string()).ok_or_else(|| "No resolved_url in response".to_string())
    }

    fn extract_decipher_ops(&self, body: &str) -> Option<Vec<DecipherOp>> {
        let func_match = self.find_decipher_function(body)?;
        let mut ops = Vec::new();
        let ops_body = &body[func_match.0..func_match.1];

        if ops_body.contains(".reverse(") || ops_body.contains("reverse(") {
            ops.push(DecipherOp::Reverse);
        }

        for cap in ops_body.split("splice") {
            if let Some(n) = cap
                .chars()
                .skip_while(|c| *c != '(')
                .skip(1)
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .ok()
            {
                ops.push(DecipherOp::Splice(n));
            }
        }

        for cap in ops_body.split("swap") {
            if let Some(n) = cap
                .chars()
                .skip_while(|c| *c != '(')
                .skip(1)
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .ok()
            {
                ops.push(DecipherOp::Swap(n));
            }
        }

        if ops.is_empty() {
            warn!(target: "YouTube-Cipher", "No decipher operations found in player script");
            return None;
        }

        info!(target: "YouTube-Cipher", "Extracted {} decipher ops: {:?}", ops.len(), ops);
        Some(ops)
    }

    fn find_decipher_function(&self, body: &str) -> Option<(usize, usize)> {
        if let Some(start) = body.find("function(") {
            if let Some(end) = body[start..].find("}") {
                let func = &body[start..start + end + 1];
                if func.contains("reverse") || func.contains("splice") {
                    return Some((start, start + end + 1));
                }
            }
        }
        if let Some(start) = body.find("=>{") {
            if let Some(end) = body[start..].find("}") {
                let func = &body[start..start + end + 1];
                if func.contains("reverse") || func.contains("splice") {
                    return Some((start, start + end + 1));
                }
            }
        }
        None
    }

    pub fn apply_ops(sig: &str, ops: &[DecipherOp]) -> String {
        let mut chars: Vec<char> = sig.chars().collect();
        for op in ops {
            match op {
                DecipherOp::Reverse => chars.reverse(),
                DecipherOp::Splice(n) => { if *n < chars.len() { chars.drain(..*n); } }
                DecipherOp::Swap(n) => { if *n < chars.len() && chars.len() > 1 { chars.swap(0, *n); } }
            }
        }
        chars.into_iter().collect()
    }

    pub fn resolve_local_url(&self, cipher_str: &str, ops: &[DecipherOp]) -> Option<String> {
        let params: HashMap<String, String> =
            url::form_urlencoded::parse(cipher_str.as_bytes())
                .into_owned()
                .collect();
        let url = params.get("url")?;
        let s = params.get("s")?;
        let sp = params.get("sp").map(|s| s.as_str()).unwrap_or("signature");

        if url.contains(sp) {
            return Some(url.to_owned());
        }

        let deciphered = Self::apply_ops(s, ops);
        let sep = if url.contains('?') { "&" } else { "?" };
        Some(format!("{url}{sep}{sp}={deciphered}"))
    }
}

impl Default for CipherManager {
    fn default() -> Self {
        Self::new()
    }
}
