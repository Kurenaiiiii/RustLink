use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::config::EternalBoxConfig;
use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{TrackData, TrackInfo};

pub struct EternalBoxSource {
    client: Client,
    base_url: String,
    max_results: usize,
}

impl EternalBoxSource {
    pub fn new(config: &EternalBoxConfig) -> Self {
        Self {
            client: Client::new(),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            max_results: config.search_results,
        }
    }

    fn extract_id(input: &str) -> Option<String> {
        if let Some(id) = input.strip_prefix("eternalbox:") {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        if let Some(id) = input.strip_prefix("holo:") {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        if input.starts_with("http://") || input.starts_with("https://") {
            return None;
        }
        if input.len() >= 10 && input.len() <= 64
            && input.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Some(input.to_string());
        }
        None
    }

    fn build_track(
        &self,
        id: &str,
        title: &str,
        author: &str,
        duration_ms: i64,
        artwork_url: Option<String>,
        isrc: Option<String>,
    ) -> TrackData {
        let jukebox_url = format!("{}/jukebox_go.html?id={}", self.base_url, id);
        let stream_url = format!("{}/api/audio/jukebox/{}", self.base_url, id);
        let analysis_url = format!("{}/api/analysis/analyse/{}", self.base_url, id);

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: id.to_string(),
                is_seekable: true,
                author: author.to_string(),
                length: duration_ms,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: Some(jukebox_url),
                artwork_url,
                isrc,
                source_name: "eternalbox".into(),
                chapters: None,
            },
            plugin_info: json!({
                "streamUrl": stream_url,
                "analysisUrl": analysis_url,
            }),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(crate::tracks::encode_track(&track));
        track
    }
}

#[async_trait]
impl SourceProvider for EternalBoxSource {
    fn name(&self) -> &'static str {
        "eternalbox"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["holo"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["eternalbox:", "holo:"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        if query.is_empty() {
            return Ok(SourceResult::Empty);
        }

        if let Some(id) = Self::extract_id(query) {
            return self.resolve(&format!("eternalbox:{id}"), None).await;
        }

        let url = format!(
            "{}/api/analysis/search?query={}&results={}",
            self.base_url,
            urlencoding(&query),
            self.max_results,
        );

        let resp = match self.client.get(&url).header("Accept", "application/json").send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(SourceResult::Empty),
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Ok(SourceResult::Empty),
        };

        let items = body.as_array()
            .or_else(|| body.get("results").and_then(|r| r.as_array()))
            .or_else(|| body.get("data").and_then(|r| r.as_array()))
            .or_else(|| body.get("tracks").and_then(|r| r.as_array()))
            .or_else(|| body.get("items").and_then(|r| r.as_array()))
            .cloned()
            .unwrap_or_default();

        let tracks: Vec<TrackData> = items.iter().filter_map(|item| {
            let info = item.get("info").or_else(|| item.get("track")).unwrap_or(item);
            let id = info.get("id").and_then(|v| v.as_str()).or_else(|| item.get("id").and_then(|v| v.as_str()))?;
            let title = info.get("title").or_else(|| info.get("name")).and_then(|v| v.as_str()).unwrap_or("Unknown Title");
            let author = info.get("artist").or_else(|| info.get("author")).and_then(|v| v.as_str()).unwrap_or("Unknown Artist");

            let duration_secs = info.get("duration")
                .or_else(|| info.get("length"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as i64;
            let artwork = info.get("artwork").or_else(|| info.get("image")).and_then(|v| v.as_str()).map(|s| s.to_string());
            let isrc = info.get("isrc").and_then(|v| v.as_str()).map(|s| s.to_string());

            Some(self.build_track(id, title, author, duration_secs * 1000, artwork, isrc))
        }).collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }
        Ok(SourceResult::Search { data: tracks })
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let id = match Self::extract_id(query) {
            Some(id) => id,
            None => return Ok(SourceResult::Empty),
        };

        let analysis_url = format!("{}/api/analysis/analyse/{}", self.base_url, id);
        let resp = match self.client.get(&analysis_url).header("Accept", "application/json").send().await {
            Ok(r) if r.status().is_success() => r,
            _ => {
                let track = self.build_track(&id, &id, "EternalBox", 0, None, None);
                return Ok(SourceResult::Track(track));
            }
        };

        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => {
                let track = self.build_track(&id, &id, "EternalBox", 0, None, None);
                return Ok(SourceResult::Track(track));
            }
        };

        let info = body.get("info");
        let title = info
            .and_then(|i| i.get("title").or_else(|| i.get("name")))
            .and_then(|v| v.as_str())
            .unwrap_or(&id);
        let author = info
            .and_then(|i| i.get("artist").or_else(|| i.get("author")))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Artist");
        let duration_secs = info
            .and_then(|i| i.get("duration").or_else(|| i.get("length")))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64;
        let artwork = info
            .and_then(|i| i.get("artwork").or_else(|| i.get("image")))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let isrc = info
            .and_then(|i| i.get("isrc"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let track = self.build_track(&id, title, author, duration_secs * 1000, artwork, isrc);
        Ok(SourceResult::Track(track))
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let id = track.identifier.as_str();
        let stream_url = format!("{}/api/audio/jukebox/{}", self.base_url, id);

        Ok(TrackUrlResult {
            url: Some(stream_url),
            protocol: Some("https".to_string()),
            format: json!("m4a"),
            new_track: None,
            additional_data: json!({}),
            exception: None,
        })
    }
}

fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}
