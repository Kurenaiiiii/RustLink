use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const URL_PATTERN: &str = r"https?://(?:t\.me|telegram\.me|telegram\.dog)/([^/]+)/(\d+)";

pub struct TelegramSource {
    client: Client,
    url_re: Regex,
    video_block_re: Regex,
    video_src_re: Regex,
    thumb_re: Regex,
    author_re: Regex,
    desc_re: Regex,
    duration_re: Regex,
}

impl TelegramSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            url_re: Regex::new(URL_PATTERN).unwrap(),
            video_block_re: Regex::new(r#"<a class="tgme_widget_message_video_player[\s\S]*?</time>"#).unwrap(),
            video_src_re: Regex::new(r#"<video[^>]+src="([^"]+)""#).unwrap(),
            thumb_re: Regex::new(r#"tgme_widget_message_video_thumb"[^>]+background-image:url\('([^']+)'\)"#).unwrap(),
            author_re: Regex::new(r#"class="tgme_widget_message_author[^>]*>[\s\S]*?<span dir="auto">([^<]+)</span>"#).unwrap(),
            desc_re: Regex::new(r#"class="tgme_widget_message_text[^>]*>([\s\S]*?)</div>"#).unwrap(),
            duration_re: Regex::new(r#"<time[^>]+duration[^>]*>([\d:]+)</time>"#).unwrap(),
        }
    }

    fn extract_author(&self, html: &str) -> String {
        self.author_re
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_else(|| "Telegram Channel".into())
    }

    fn extract_description(&self, html: &str) -> String {
        let text = self
            .desc_re
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        if text.is_empty() {
            return text;
        }

        let text = text
            .replace("<br />", "\n")
            .replace("<br/>", "\n")
            .replace("<br>", "\n");

        let mut result = String::with_capacity(text.len());
        let mut in_tag = false;
        for c in text.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ => {
                    if !in_tag {
                        result.push(c);
                    }
                }
            }
        }
        result.trim().to_string()
    }

    fn parse_duration_ms(&self, content: &str) -> i64 {
        let dur = self
            .duration_re
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .unwrap_or("");

        let parts: Vec<&str> = dur.split(':').collect();
        match parts.len() {
            3 => {
                let h = parts[0].parse::<i64>().unwrap_or(0);
                let m = parts[1].parse::<i64>().unwrap_or(0);
                let s = parts[2].parse::<i64>().unwrap_or(0);
                (h * 3600 + m * 60 + s) * 1000
            }
            2 => {
                let m = parts[0].parse::<i64>().unwrap_or(0);
                let s = parts[1].parse::<i64>().unwrap_or(0);
                (m * 60 + s) * 1000
            }
            _ => 0,
        }
    }

    fn build_track(
        &self,
        identifier: String,
        title: String,
        author: String,
        length: i64,
        uri: String,
        artwork_url: Option<String>,
        video_url: String,
    ) -> TrackData {
        let info = TrackInfo {
            identifier,
            is_seekable: true,
            author,
            length,
            is_stream: false,
            position: 0,
            title,
            uri: Some(uri),
            artwork_url,
            isrc: None,
            source_name: "telegram".into(),
            chapters: None,
        };
        let mut track = TrackData {
            encoded: None,
            info,
            plugin_info: serde_json::json!({ "directUrl": video_url }),
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        track
    }
}

#[async_trait]
impl SourceProvider for TelegramSource {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &[]
    }

    async fn search(&self, _query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        Ok(SourceResult::Empty)
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let caps = match self.url_re.captures(query) {
            Some(c) => c,
            None => return Ok(SourceResult::Empty),
        };

        let channel = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let message_id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if channel.is_empty() || message_id.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let embed_url = if query.contains('?') {
            format!("{query}&embed=1")
        } else {
            format!("{query}?embed=1")
        };

        let resp = self
            .client
            .get(&embed_url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header("Accept-Encoding", "identity")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(SourceResult::Empty);
        }

        let html = resp.text().await?;
        let author = self.extract_author(&html);
        let desc = self.extract_description(&html);
        let title = desc.lines().next().unwrap_or("").to_string();
        let title = if title.is_empty() {
            format!("Telegram Video {message_id}")
        } else {
            title
        };

        let blocks: Vec<&str> = self
            .video_block_re
            .find_iter(&html)
            .map(|m| m.as_str())
            .collect();

        if blocks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let tracks: Vec<TrackData> = blocks
            .iter()
            .enumerate()
            .filter_map(|(i, block)| {
                let video_url = self
                    .video_src_re
                    .captures(block)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str())?;

                let thumb = self
                    .thumb_re
                    .captures(block)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string());

                let duration = self.parse_duration_ms(block);
                let track_title = if i == 0 {
                    title.clone()
                } else {
                    format!("{title} (Video {})", i + 1)
                };

                Some(self.build_track(
                    format!("{channel}/{message_id}/{i}"),
                    track_title,
                    author.clone(),
                    duration,
                    query.to_string(),
                    thumb,
                    video_url.to_string(),
                ))
            })
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let is_single = query.contains("?single") || query.contains("&single");
        if is_single || tracks.len() == 1 {
            return Ok(SourceResult::Track(
                tracks.into_iter().next().unwrap(),
            ));
        }

        let encoded = tracks[0].encoded.clone().unwrap_or_default();
        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded,
                info: PlaylistInfo {
                    name: title,
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({}),
                tracks,
            },
        })
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let uri = match &track.uri {
            Some(u) => u,
            None => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some("No URI in track".into()),
                })
            }
        };

        match self.resolve(uri, None).await? {
            SourceResult::Track(t) => {
                let url = t
                    .plugin_info
                    .get("directUrl")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Ok(TrackUrlResult {
                    url,
                    protocol: Some("https".into()),
                    format: serde_json::json!("mp4"),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: None,
                })
            }
            SourceResult::Playlist { data: pl, .. } => {
                let parts: Vec<&str> = track.identifier.split('/').collect();
                let idx: usize = parts
                    .last()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let selected = pl.tracks.get(idx).or_else(|| pl.tracks.first());
                match selected {
                    Some(t) => {
                        let url = t
                            .plugin_info
                            .get("directUrl")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        Ok(TrackUrlResult {
                            url,
                            protocol: Some("https".into()),
                            format: serde_json::json!("mp4"),
                            new_track: None,
                            additional_data: serde_json::json!({}),
                            exception: None,
                        })
                    }
                    None => Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: serde_json::json!({}),
                        new_track: None,
                        additional_data: serde_json::json!({}),
                        exception: Some("Track not found in playlist".into()),
                    }),
                }
            }
            _ => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: serde_json::json!({}),
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: Some("Failed to resolve Telegram track".into()),
            }),
        }
    }
}
