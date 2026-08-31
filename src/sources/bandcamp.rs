use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

pub struct BandcampSource {
    client: Client,
}

impl BandcampSource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .unwrap(),
        }
    }

    fn parse_bandcamp_page(html: &str) -> Option<(TrackData, Option<String>)> {
        // Look for embedded track data in bandcamp.com pages
        // Pattern 1: data-tralbum JSON blob
        // Pattern 2: <script> with trackinfo
        let trackinfo_marker = r#""trackinfo":"#;
        let pos = html.find(trackinfo_marker)?;
        let start = pos + trackinfo_marker.len();
        let mut depth = 0;
        let mut end = start;
        let bytes = html.as_bytes();
        for i in start..bytes.len() {
            match bytes[i] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth != 0 || end <= start {
            return None;
        }

        let trackinfo_str = &html[start..end];
        let trackinfo: Value = serde_json::from_str(trackinfo_str).ok()?;
        let first = trackinfo.as_array()?.first()?;

        let title = first.get("title").and_then(|t| t.as_str())?;
        let duration_sec = first.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0);
        let audio_url = first.get("file").and_then(|f| f.as_str());

        // Extract artist from page title or other metadata
        let artist = html
            .split_once(r#""artist":"#)
            .and_then(|(_, rest)| rest.split('"').next())
            .unwrap_or("Unknown Artist")
            .to_string();

        // Extract album art
        let art_url: Option<String> = html
            .split_once(r#""artFullsizeUrl":"#)
            .and_then(|(_, rest)| rest.split('"').next().map(|s| s.to_string()))
            .or_else(|| {
                html.split_once(r#""artId":"#)
                    .and_then(|(_, rest)| {
                        let id = rest.split('"').next()?;
                        Some(format!("https://f4.bcbits.com/img/a{id}_16.jpg"))
                    })
            });

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: title.to_string(),
                is_seekable: true,
                author: artist,
                length: (duration_sec * 1000.0) as i64,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: None,
                artwork_url: art_url,
                isrc: None,
                source_name: "bandcamp".to_string(),
                chapters: None,
            },
            plugin_info: serde_json::json!({}),
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));

        Some((track, audio_url.map(|s| s.to_string())))
    }
}

#[async_trait]
impl SourceProvider for BandcampSource {
    fn name(&self) -> &'static str {
        "bandcamp"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["bc", "bandcamp"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let url = format!("https://bandcamp.com/search?q={encoded}&item_type=t");
        let resp = self.client.get(&url).send().await?;
        let html = resp.text().await?;

        let mut tracks: Vec<TrackData> = Vec::new();
        // Parse search result items
        for result_item in html.split(r#"<div class="result-items">"#).skip(1) {
            let item = result_item.split(r#"</div>"#).next().unwrap_or(result_item);
            let title = item
                .split(r#"<div class="heading">"#)
                .nth(1)
                .and_then(|s| s.split("</div>").next())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let artist = item
                .split(r#"<div class="subhead">"#)
                .nth(1)
                .and_then(|s| s.split("</div>").next())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let url_href = item
                .split(r#"href=""#)
                .nth(1)
                .and_then(|s| s.split('"').next())
                .map(|s| s.to_string());

            if title.is_empty() || artist.is_empty() || url_href.is_none() {
                continue;
            }

            let mut track = TrackData {
                encoded: None,
                info: TrackInfo {
                    identifier: url_href.clone().unwrap_or_default(),
                    is_seekable: true,
                    author: artist,
                    length: 0,
                    is_stream: false,
                    position: 0,
                    title,
                    uri: url_href,
                    artwork_url: None,
                    isrc: None,
                    source_name: "bandcamp".to_string(),
                    chapters: None,
                },
                plugin_info: serde_json::json!({}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            track.encoded = Some(encode_track(&track));
            tracks.push(track);
        }

        if tracks.is_empty() {
            Ok(SourceResult::Empty)
        } else {
            Ok(SourceResult::Search { data: tracks })
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let trimmed = query.trim();
        let url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else if trimmed.starts_with("bandcamp:") {
            format!("https://{}.bandcamp.com", &trimmed["bandcamp:".len()..])
        } else {
            return self.search(trimmed, None).await;
        };

        info!(target: "Bandcamp", "Resolving URL: {url}");

        let resp = match self.client.get(&url).header("User-Agent", "Mozilla/5.0").send().await {
            Ok(r) => {
                if r.status().is_success() {
                    r
                } else {
                    warn!(target: "Bandcamp", "HTTP {} for {}", r.status(), url);
                    return Ok(SourceResult::Empty);
                }
            }
            Err(e) => {
                warn!(target: "Bandcamp", "Request error: {e}");
                return Ok(SourceResult::Empty);
            }
        };

        let html = match resp.text().await {
            Ok(h) => h,
            Err(e) => {
                warn!(target: "Bandcamp", "Read body error: {e}");
                return Ok(SourceResult::Empty);
            }
        };

        match Self::parse_bandcamp_page(&html) {
            Some((track, _)) => Ok(SourceResult::Track(track)),
            None => {
                warn!(target: "Bandcamp", "Failed to parse page: {url}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        // Resolve the URL to get the audio URL from the page
        let url = track.uri.as_deref().unwrap_or(&track.identifier);
        if url.is_empty() {
            return Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: serde_json::json!({}),
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: Some("Bandcamp: no URL to resolve".into()),
            });
        }

        let resp = match self.client.get(url).header("User-Agent", "Mozilla/5.0").send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some(format!("Bandcamp: request failed: {e}")),
                });
            }
        };

        let html = match resp.text().await {
            Ok(h) => h,
            Err(e) => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some(format!("Bandcamp: read error: {e}")),
                });
            }
        };

        match Self::parse_bandcamp_page(&html) {
            Some((_, audio_url)) => {
                if let Some(url) = audio_url {
                    Ok(TrackUrlResult {
                        url: Some(url),
                        protocol: Some("https".into()),
                        format: serde_json::json!({"protocol": "https"}),
                        new_track: None,
                        additional_data: serde_json::json!({}),
                        exception: None,
                    })
                } else {
                    Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: serde_json::json!({}),
                        new_track: None,
                        additional_data: serde_json::json!({}),
                        exception: Some("Bandcamp: no audio URL found on page".into()),
                    })
                }
            }
            None => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: serde_json::json!({}),
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: Some("Bandcamp: failed to parse page".into()),
            }),
        }
    }
}
