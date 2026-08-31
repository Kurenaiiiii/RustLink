//! YouTube PO token manager backed by an embedded BotGuard runtime.
//!
//! Mirrors NodeLink's PoTokenManager (src/sources/youtube/sabr/potoken.ts):
//!   1. Fetch a real VISITOR_DATA from the YouTube home page HTML.
//!   2. Run YouTube's BotGuard VM challenge (via rustypipe-botguard, an embedded
//!      Deno JS runtime — the same BgUtils reverse-engineering NodeLink builds on).
//!   3. Mint session-bound tokens (identifier = visitorData) for streaming/SABR
//!      and content-bound tokens (identifier = videoId) for player requests.
//!
//! The Botguard instance is !Send, so it lives on a dedicated OS thread with its
//! own single-threaded tokio runtime and is fed identifiers over an mpsc channel.

use base64::Engine;
use rustypipe_botguard::Botguard;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

const CHROME_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
/// Refresh any minted token this many seconds before its reported expiry.
const EXPIRY_MARGIN_SECS: i64 = 300;
const CONTENT_CACHE_CAP: usize = 128;

#[derive(Debug, Clone)]
struct MintedToken {
    token: String,
    /// Unix timestamp after which the token should not be trusted anymore.
    valid_until: i64,
}

enum BotguardRequest {
    Mint {
        identifier: String,
        response: oneshot::Sender<Result<MintedToken, String>>,
    },
}

struct PoTokenInner {
    po_token: tokio::sync::Mutex<Option<MintedToken>>,
    visitor_data: Mutex<String>,
    /// Set when a token came from config or the polling endpoint — those take
    /// precedence and must never be replaced by locally minted ones.
    has_external_token: AtomicBool,
    po_token_endpoint: Option<String>,
    http_client: reqwest::Client,
    botguard_tx: Mutex<Option<mpsc::Sender<BotguardRequest>>>,
    /// Serializes lazy initialization (visitor data fetch + first mint).
    init_lock: tokio::sync::Mutex<()>,
    content_cache: Mutex<HashMap<String, MintedToken>>,
}

#[derive(Clone)]
pub struct PoTokenManager {
    inner: Arc<PoTokenInner>,
}

impl PoTokenManager {
    pub fn new(
        potoken: Option<String>,
        po_token_endpoint: Option<String>,
        http_client: reqwest::Client,
    ) -> Self {
        let has_external = potoken.is_some();
        let stored = potoken.map(|token| MintedToken {
            token,
            valid_until: now_secs() + 6 * 3600,
        });
            Self {
            inner: Arc::new(PoTokenInner {
                po_token: tokio::sync::Mutex::new(stored),
                visitor_data: Mutex::new(String::new()),
                has_external_token: AtomicBool::new(has_external),
                po_token_endpoint,
                http_client,
                botguard_tx: Mutex::new(None),
                init_lock: tokio::sync::Mutex::new(()),
                content_cache: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn visitor_data(&self) -> String {
        self.inner.visitor_data.lock().unwrap().clone()
    }

    /// Raw cached session token without triggering initialization.
    pub async fn get_token(&self) -> Option<String> {
        self.inner.po_token.lock().await.as_ref().map(|m| m.token.clone())
    }

    pub async fn set_token(&self, token: String) {
        self.inner.has_external_token.store(true, Ordering::Relaxed);
        *self.inner.po_token.lock().await = Some(MintedToken {
            token,
            valid_until: now_secs() + 6 * 3600,
        });
    }

    /// Returns a session-bound PO token, initializing BotGuard on first use.
    ///
    /// This is the lazy equivalent of NodeLink's `initialize()` +
    /// `generateStreamingToken()`: scrape visitor data, solve the BotGuard
    /// challenge once, then mint a token bound to the visitor data.
    pub async fn try_generate(&self) -> Option<String> {
        // Fast path: unexpired cached token.
        if let Some(token) = self.fresh_session_token().await {
            return Some(token);
        }

        let _guard = self.inner.init_lock.lock().await;

        // Re-check under the init lock.
        if let Some(token) = self.fresh_session_token().await {
            return Some(token);
        }

        // An explicitly configured token wins; never overwrite it.
        if self.inner.has_external_token.load(Ordering::Relaxed) {
            let cached = self.inner.po_token.lock().await.as_ref().map(|m| m.token.clone());
            if cached.is_some() {
                return cached;
            }
        }

        // Obtain a real visitor data (NodeLink: fetchVisitorData()).
        let mut vd = self.visitor_data();
        if vd.is_empty() {
            match fetch_visitor_data(&self.inner.http_client).await {
                Ok(real_vd) => {
                    tracing::info!(
                        target: "PoToken",
                        "Fetched visitorData from YouTube ({} chars)",
                        real_vd.len()
                    );
                    vd = real_vd;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "PoToken",
                        "Could not scrape visitorData ({e}); falling back to random ID"
                    );
                    vd = generate_visitor_data();
                }
            }
            *self.inner.visitor_data.lock().unwrap() = vd.clone();
        }

        // Solve BotGuard once and mint the session-bound token.
        let tx = self.ensure_botguard_actor()?;
        let (response_tx, response_rx) = oneshot::channel();
        tx.send(BotguardRequest::Mint {
            identifier: vd,
            response: response_tx,
        })
        .await
        .ok()?;

        let minted = response_rx.await.ok()?.ok()?;
        tracing::info!(
            target: "PoToken",
            "Session PO token ready (len {}, valid for {} min)",
            minted.token.len(),
            (minted.valid_until - now_secs()).max(0) / 60
        );
        *self.inner.po_token.lock().await = Some(minted);

        self.inner.po_token.lock().await.as_ref().map(|m| m.token.clone())
    }

    /// Token for `serviceIntegrityDimensions.poToken` on player requests.
    ///
    /// NodeLink mints a content-bound token per video ID; falls back to the
    /// session token when BotGuard is unavailable.
    pub async fn token_for_player_request(&self, video_id: Option<&str>) -> Option<String> {
        let Some(video_id) = video_id else {
            return self.try_generate().await;
        };

        // Make sure BotGuard + visitor data exist first.
        if self.try_generate().await.is_none() {
            return None;
        }
        if self.inner.has_external_token.load(Ordering::Relaxed) {
            // External tokens are not mintable per-content; use as-is.
            return self.get_token().await;
        }

        if let Some(cached) = self.inner.content_cache.lock().unwrap().get(video_id) {
            if cached.valid_until - EXPIRY_MARGIN_SECS > now_secs() {
                return Some(cached.token.clone());
            }
        }

        let tx = self.ensure_botguard_actor()?;
        let (response_tx, response_rx) = oneshot::channel();
        tx.send(BotguardRequest::Mint {
            identifier: video_id.to_string(),
            response: response_tx,
        })
        .await
        .ok()?;

        let minted = response_rx.await.ok()?.ok()?;
        tracing::debug!(
            target: "PoToken",
            "Content PO token minted for video {}",
            video_id
        );

        let mut cache = self.inner.content_cache.lock().unwrap();
        if cache.len() >= CONTENT_CACHE_CAP {
            cache.clear();
        }
        cache.insert(video_id.to_string(), minted);
        cache.get(video_id).map(|m| m.token.clone())
    }

    /// Re-fetches visitor data from YouTube. Invalidates minted tokens because
    /// they are bound to the old visitor data (NodeLink does the same reset).
    pub async fn refresh_visitor_data(&self) {
        if self.inner.has_external_token.load(Ordering::Relaxed) {
            return;
        }
        let Ok(new_vd) = fetch_visitor_data(&self.inner.http_client).await else {
            tracing::warn!(target: "PoToken", "Visitor data refresh failed; keeping previous");
            return;
        };
        tracing::info!(target: "PoToken", "Visitor data rotated — resetting minted tokens");
        {
            let mut vd_guard = self.inner.visitor_data.lock().unwrap();
            if *vd_guard == new_vd {
                return;
            }
            *vd_guard = new_vd;
        }

        *self.inner.po_token.lock().await = None;
        self.inner.content_cache.lock().unwrap().clear();
    }

    /// Spawns a background task that polls the PoToken endpoint periodically.
    /// The endpoint should return JSON like {"poToken": "...", "visitorData": "..."}.
    pub fn start_polling(&self) -> Option<tokio::task::JoinHandle<()>> {
        let endpoint = self.inner.po_token_endpoint.clone()?;
        let inner = self.inner.clone();

        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                match inner.http_client.get(&endpoint).send().await {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(json) => {
                            let mut got_token = false;
                            if let Some(token) = json.get("poToken").and_then(|v| v.as_str()) {
                                *inner.po_token.lock().await = Some(MintedToken {
                                    token: token.to_string(),
                                    valid_until: now_secs() + 6 * 3600,
                                });
                                inner.has_external_token.store(true, Ordering::Relaxed);
                                got_token = true;
                            }
                            if let Some(vd) = json.get("visitorData").and_then(|v| v.as_str()) {
                                *inner.visitor_data.lock().unwrap() = vd.to_string();
                            }
                            if got_token {
                                tracing::info!(target: "PoToken", "Loaded PO token from endpoint");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(target: "PoToken", "Failed to parse PoToken response: {e}");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(target: "PoToken", "Failed to fetch PoToken from endpoint: {e}");
                    }
                }
            }
        }))
    }

    async fn fresh_session_token(&self) -> Option<String> {
        let guard = self.inner.po_token.lock().await;
        guard
            .as_ref()
            .filter(|m| m.valid_until - EXPIRY_MARGIN_SECS > now_secs())
            .map(|m| m.token.clone())
    }

    fn ensure_botguard_actor(&self) -> Option<mpsc::Sender<BotguardRequest>> {
        let mut slot = self.inner.botguard_tx.lock().unwrap();
        if slot.is_none() {
            tracing::info!(target: "PoToken", "Starting embedded BotGuard runtime…");
            *slot = Some(spawn_botguard_actor());
        }
        slot.clone()
    }
}

impl Default for PoTokenManager {
    fn default() -> Self {
        Self::new(None, None, reqwest::Client::new())
    }
}

fn spawn_botguard_actor() -> mpsc::Sender<BotguardRequest> {
    let (tx, mut rx) = mpsc::channel::<BotguardRequest>(16);

    std::thread::Builder::new()
        .name("rustlink-botguard".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                let mut instance: Option<Botguard> = None;
                while let Some(request) = rx.recv().await {
                    let BotguardRequest::Mint {
                        identifier,
                        response,
                    } = request;

                    let expired = instance
                        .as_ref()
                        .map(|bg| bg.valid_until().unix_timestamp() - EXPIRY_MARGIN_SECS <= now_secs())
                        .unwrap_or(true);

                    if expired {
                        match init_botguard().await {
                            Ok(bg) => instance = Some(bg),
                            Err(e) => {
                                tracing::error!(target: "PoToken", "BotGuard init failed: {e}");
                                let _ = response.send(Err(format!("BotGuard init failed: {e}")));
                                continue;
                            }
                        }
                    }

                    let Some(bg) = instance.as_mut() else {
                        let _ = response.send(Err("BotGuard not initialized".into()));
                        continue;
                    };

                    match bg.mint_token(&identifier).await {
                        Ok(token) => {
                            let valid_until = bg.valid_until().unix_timestamp();
                            let _ = response.send(Ok(MintedToken {
                                token,
                                valid_until,
                            }));
                        }
                        Err(e) => {
                            // A failed mint usually means the integrity token
                            // went stale — force a fresh challenge next time.
                            instance = None;
                            let _ = response.send(Err(format!("mint failed: {e}")));
                        }
                    }
                }
            });
        })
        .ok();

    tx
}

async fn init_botguard() -> anyhow::Result<Botguard> {
    // Keep the PathBuf alive for the duration of the call — the builder only
    // holds a reference.
    let snapshot = snapshot_path();
    let mut builder = Botguard::builder().user_agent(CHROME_UA);
    if let Some(path) = snapshot.as_deref() {
        builder = builder.snapshot_path_opt(Some(path));
    }

    let started = Instant::now();
    let bg = builder.init().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(
        target: "PoToken",
        "BotGuard challenge solved in {:.1}s (from_snapshot: {})",
        started.elapsed().as_secs_f32(),
        bg.is_from_snapshot()
    );
    Ok(bg)
}

/// Scrapes a real VISITOR_DATA out of the YouTube home page, exactly like
/// NodeLink's `fetchVisitorData()`.
async fn fetch_visitor_data(http_client: &reqwest::Client) -> anyhow::Result<String> {
    const MARKER: &str = "\"VISITOR_DATA\":\"";

    let html = http_client
        .get("https://www.youtube.com")
        .header(reqwest::header::USER_AGENT, CHROME_UA)
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .text()
        .await?;

    let start = html
        .find(MARKER)
        .ok_or_else(|| anyhow::anyhow!("VISITOR_DATA not found in home page HTML"))?
        + MARKER.len();
    let end = html[start..]
        .find('"')
        .map(|i| start + i)
        .ok_or_else(|| anyhow::anyhow!("unterminated VISITOR_DATA value"))?;

    Ok(html[start..end].to_string())
}

fn snapshot_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(|home| {
            let dir = PathBuf::from(home).join(".local/share/rustlink");
            let _ = std::fs::create_dir_all(&dir);
            dir.join("bg_snapshot.bin")
        })
    }
    #[cfg(not(unix))]
    {
        Some(std::env::temp_dir().join("rustlink_bg_snapshot.bin"))
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn generate_visitor_data() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..24).map(|_| rng.gen()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
