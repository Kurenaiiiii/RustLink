use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const FALLBACK_CLIENT_ID: &str = "N8DkIx0V2x7D8BC5J7vNxLIR3rT1eS9v";
const DEFAULT_SEARCH_LIMIT: u32 = 10;

pub struct SoundCloudSource {
    client: Client,
    client_id: Arc<RwLock<String>>,
}

impl SoundCloudSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            client_id: Arc::new(RwLock::new(FALLBACK_CLIENT_ID.to_string())),
        }
    }

    async fn refresh_client_id(&self) -> String {
        match self.extract_client_id().await {
            Some(id) => {
                info!(target: "SoundCloud", "Extracted client_id from soundcloud.com");
                let mut stored = self.client_id.write().await;
                *stored = id.clone();
                id
            }
            None => {
                warn!(target: "SoundCloud", "Failed to extract client_id, using fallback");
                self.client_id.read().await.clone()
            }
        }
    }

    async fn extract_client_id(&self) -> Option<String> {
        let html = self
            .client
            .get("https://soundcloud.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;

        // Try to find client_id directly in the page HTML first
        // Pattern: client_id:"..." or clientId:"..."
        for marker in &["client_id:\"", "clientId:\"", "\"clientId\":\""] {
            if let Some(start) = html.find(marker) {
                let val_start = start + marker.len();
                let remaining = &html[val_start..];
                let end = remaining.find('"')?;
                let id = &remaining[..end];
                if !id.is_empty() && id.len() < 64 && id.chars().all(|c| c.is_alphanumeric()) {
                    return Some(id.to_string());
                }
            }
        }

        // Fallback: find script src containing "sounds" or "webpack", fetch and search JS
        let mut search_from = 0usize;
        loop {
            let script_idx = html[search_from..].find("<script")? + search_from;
            let tag_end = html[script_idx..].find('>')? + script_idx;
            let tag = &html[script_idx..tag_end];

            if let Some(src_pos) = tag.find("src=\"") {
                let val_start = src_pos + 5;
                let src_end = tag[val_start..].find('"')?;
                let src = &tag[val_start..val_start + src_end];

                if (src.contains("sounds") || src.contains("webpack")) && src.contains("http") {
                    if let Ok(script) = self
                        .client
                        .get(src)
                        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                        .send()
                        .await
                    {
                        if let Ok(body) = script.text().await {
                            let marker2 = "client_id:\"";
                            if let Some(ci_start) = body.find(marker2) {
                                let val_start2 = ci_start + marker2.len();
                                let val_end = body[val_start2..].find('"')?;
                                let id = &body[val_start2..val_start2 + val_end];
                                if !id.is_empty() && id.len() < 64 {
                                    return Some(id.to_string());
                                }
                            }
                        }
                    }
                }
            }

            search_from = tag_end + 1;
        }
    }

    async fn api_get(&self, path: &str, params: Vec<(&str, String)>) -> anyhow::Result<Value> {
        let cid = self.client_id.read().await.clone();
        let mut all_params = params.clone();
        all_params.push(("client_id", cid));

        let resp = self
            .client
            .get(format!("https://api-v2.soundcloud.com{path}"))
            .query(&all_params)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await?;

        if resp.status().as_u16() == 401 {
            let new_cid = self.refresh_client_id().await;
            let mut refreshed_params = params;
            refreshed_params.push(("client_id", new_cid));
            let resp = self
                .client
                .get(format!("https://api-v2.soundcloud.com{path}"))
                .query(&refreshed_params)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .send()
                .await?;
            return Ok(resp.error_for_status()?.json().await?);
        }

        Ok(resp.error_for_status()?.json().await?)
    }

    fn parse_soundcloud_track(item: &Value) -> Option<TrackData> {
        let kind = item.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if kind != "track" {
            return None;
        }

        let id = item.get("id").and_then(|v| v.as_i64())?;
        let title = item.get("title").and_then(|v| v.as_str())?;
        let author = item
            .get("user")
            .and_then(|u| u.get("username"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let duration = item.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
        let uri = item
            .get("permalink_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let artwork = item
            .get("artwork_url")
            .and_then(|v| v.as_str())
            .map(|s| s.replace("-large.", "-t500x500."));

        let track_info = TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: author.to_string(),
            length: duration,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri,
            artwork_url: artwork,
            isrc: item
                .get("isrc")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            source_name: "soundcloud".to_string(),
            chapters: None,
        };

        let mut track_data = TrackData {
            encoded: None,
            info: track_info,
            plugin_info: serde_json::json!({}),
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track_data.encoded = Some(encode_track(&track_data));
        Some(track_data)
    }

    fn parse_playlist(item: &Value) -> Option<SourceResult> {
        let kind = item.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if kind != "playlist" && kind != "album" {
            return None;
        }

        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Playlist")
            .to_string();
        let mut tracks: Vec<TrackData> = Vec::new();
        if let Some(track_list) = item.get("tracks").and_then(|v| v.as_array()) {
            for t in track_list {
                if let Some(td) = Self::parse_soundcloud_track(t) {
                    tracks.push(td);
                }
            }
        }

        if tracks.is_empty() {
            return None;
        }

        let encoded = tracks[0].encoded.clone().unwrap_or_default();
        Some(SourceResult::Playlist {
            data: crate::sources::PlaylistData {
                encoded,
                info: crate::sources::PlaylistInfo {
                    name: title,
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({}),
                tracks,
            },
        })
    }

    async fn resolve_url(&self, url: &str) -> AnyhowResult<SourceResult> {
        let data = self
            .api_get("/resolve", vec![("url", url.to_string())])
            .await;

        match data {
            Ok(json) => {
                let kind = json.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                match kind {
                    "track" => {
                        if let Some(td) = Self::parse_soundcloud_track(&json) {
                            Ok(SourceResult::Track(td))
                        } else {
                            Ok(SourceResult::Empty)
                        }
                    }
                    "playlist" | "album" => {
                        if let Some(pl) = Self::parse_playlist(&json) {
                            Ok(pl)
                        } else {
                            Ok(SourceResult::Empty)
                        }
                    }
                    _ => Ok(SourceResult::Empty),
                }
            }
            Err(e) => {
                warn!(target: "SoundCloud", "Resolve error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }
}

#[async_trait]
impl SourceProvider for SoundCloudSource {
    fn name(&self) -> &'static str {
        "soundcloud"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["sc"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["scsearch", "soundcloud"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let data = self
            .api_get(
                "/search/tracks",
                vec![("q", query.to_string()), ("limit", DEFAULT_SEARCH_LIMIT.to_string())],
            )
            .await;

        match data {
            Ok(json) => {
                let mut tracks: Vec<TrackData> = Vec::new();
                if let Some(collection) = json.get("collection").and_then(|v| v.as_array()) {
                    for item in collection {
                        if let Some(td) = Self::parse_soundcloud_track(item) {
                            tracks.push(td);
                        }
                    }
                }
                if tracks.is_empty() {
                    Ok(SourceResult::Empty)
                } else {
                    Ok(SourceResult::Search { data: tracks })
                }
            }
            Err(e) => {
                warn!(target: "SoundCloud", "Search error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let trimmed = query.trim();

        if trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("soundcloud:")
        {
            let url = if trimmed.starts_with("soundcloud:") {
                format!("https://soundcloud.com/{}", &trimmed["soundcloud:".len()..])
            } else {
                trimmed.to_string()
            };
            return self.resolve_url(&url).await;
        }

        // Treat as search
        self.search(trimmed, None).await
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let track_id = &track.identifier;

        let data = self
            .api_get(
                &format!("/tracks/{track_id}"),
                vec![],
            )
            .await;

        let json = match data {
            Ok(j) => j,
            Err(e) => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some(format!("SoundCloud: failed to fetch track data: {e}")),
                });
            }
        };

        let media = match json.get("media").and_then(|m| m.as_array()) {
            Some(m) if !m.is_empty() => m,
            _ => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some("SoundCloud: no media transcodings found".into()),
                });
            }
        };

        // Prefer progressive format, fallback to hls
        let transcoding = media
            .iter()
            .find(|t| {
                t.get("format")
                    .and_then(|f| f.get("protocol"))
                    .and_then(|p| p.as_str())
                    == Some("progressive")
            })
            .or_else(|| {
                media.iter().find(|t| {
                    t.get("format")
                        .and_then(|f| f.get("protocol"))
                        .and_then(|p| p.as_str())
                        == Some("hls")
                })
            });

        let transcoding = match transcoding {
            Some(t) => t,
            None => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some("SoundCloud: no playable transcoding found".into()),
                });
            }
        };

        let transcoding_url = match transcoding.get("url").and_then(|u| u.as_str()) {
            Some(u) => u.to_string(),
            None => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some("SoundCloud: transcoding has no URL".into()),
                });
            }
        };

        let cid = self.client_id.read().await.clone();
        let stream_resp = self
            .client
            .get(&transcoding_url)
            .query(&[("client_id", cid.as_str())])
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await;

        let stream_data: Value = match stream_resp {
            Ok(r) => {
                if r.status().as_u16() == 401 {
                    let new_cid = self.refresh_client_id().await;
                    match self
                        .client
                        .get(&transcoding_url)
                        .query(&[("client_id", new_cid.as_str())])
                        .header("User-Agent", "Mozilla/5.0")
                        .send()
                        .await
                    {
                        Ok(r2) => match r2.json().await {
                            Ok(j) => j,
                            Err(e) => {
                                return Ok(TrackUrlResult {
                                    url: None,
                                    protocol: None,
                                    format: serde_json::json!({}),
                                    new_track: None,
                                    additional_data: serde_json::json!({}),
                                    exception: Some(format!("SoundCloud: stream JSON parse error: {e}")),
                                });
                            }
                        },
                        Err(e) => {
                            return Ok(TrackUrlResult {
                                url: None,
                                protocol: None,
                                format: serde_json::json!({}),
                                new_track: None,
                                additional_data: serde_json::json!({}),
                                exception: Some(format!("SoundCloud: stream request failed: {e}")),
                            });
                        }
                    }
                } else {
                    match r.json().await {
                        Ok(j) => j,
                        Err(e) => {
                            return Ok(TrackUrlResult {
                                url: None,
                                protocol: None,
                                format: serde_json::json!({}),
                                new_track: None,
                                additional_data: serde_json::json!({}),
                                exception: Some(format!("SoundCloud: stream JSON parse error: {e}")),
                            });
                        }
                    }
                }
            }
            Err(e) => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some(format!("SoundCloud: stream request failed: {e}")),
                });
            }
        };

        let stream_url = stream_data
            .get("url")
            .and_then(|u| u.as_str())
            .map(|s| s.to_string());

        let protocol = transcoding
            .get("format")
            .and_then(|f| f.get("protocol"))
            .and_then(|p| p.as_str())
            .map(|s| s.to_string());

        let format = transcoding
            .get("format")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        if stream_url.is_none() {
            return Ok(TrackUrlResult {
                url: None,
                protocol,
                format,
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: Some("SoundCloud: stream URL not found in response".into()),
            });
        }

        Ok(TrackUrlResult {
            url: stream_url,
            protocol,
            format,
            new_track: None,
            additional_data: serde_json::json!({}),
            exception: None,
        })
    }
}

type AnyhowResult<T> = anyhow::Result<T>;
