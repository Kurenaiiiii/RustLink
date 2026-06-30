use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/58.0.3029.110 Safari/537.3";
const REDDIT_BASE: &str = "https://www.reddit.com";

pub struct RedditSource {
    client: Client,
    comments_re: Regex,
    video_re: Regex,
    share_re: Regex,
}

impl RedditSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            comments_re: Regex::new(r"/comments/([^/?]+)").unwrap(),
            video_re: Regex::new(r"/video/([^/?]+)").unwrap(),
            share_re: Regex::new(r"/r/([^/]+)/s/([^/?]+)").unwrap(),
        }
    }

    async fn resolve_redirect(&self, url: &str) -> Option<String> {
        let resp = self
            .client
            .head(url)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .ok()?;
        let location = resp.headers().get("location")?.to_str().ok()?;
        let final_url = if location.starts_with("http") {
            location.to_string()
        } else {
            format!("{REDDIT_BASE}{location}")
        };
        self.comments_re
            .captures(&final_url)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    }

    async fn find_audio_url(&self, video_url: &str) -> Option<String> {
        let base = video_url.split('_').next().unwrap_or(video_url);
        let dash_prefix = video_url.split("DASH").next().unwrap_or(video_url);
        let variants = if video_url.contains(".mp4") {
            vec![
                format!("{base}_audio.mp4"),
                format!("{base}_AUDIO_128.mp4"),
                format!("{dash_prefix}audio"),
                format!("{base}_audio.mp3"),
                format!("{base}_AUDIO_128.mp3"),
            ]
        } else {
            vec![
                format!("{}audio", dash_prefix),
                format!("{base}_AUDIO_128.mp4"),
                format!("{base}_audio.mp3"),
                format!("{base}_AUDIO_128.mp3"),
            ]
        };

        for url in &variants {
            if let Ok(resp) = self.client.head(url).send().await {
                if resp.status().is_success() {
                    return Some(url.clone());
                }
            }
        }
        None
    }

    fn parse_url(&self, url: &str) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
        // Returns (id, short_id, sub, share_id)
        if let Some(caps) = self.video_re.captures(url) {
            if let Some(id) = caps.get(1) {
                return (None, Some(id.as_str().to_string()), None, None);
            }
        }
        if let Some(caps) = self.comments_re.captures(url) {
            if let Some(id) = caps.get(1) {
                return (Some(id.as_str().to_string()), None, None, None);
            }
        }
        if let Some(caps) = self.share_re.captures(url) {
            let sub = caps.get(1).map(|m| m.as_str().to_string());
            let share_id = caps.get(2).map(|m| m.as_str().to_string());
            return (None, None, sub, share_id);
        }
        (None, None, None, None)
    }

    async fn get_reddit_track(&self, url: &str) -> Option<TrackData> {
        let (mut id, short_id, sub, share_id) = self.parse_url(url);

        // Resolve short IDs and share URLs
        if id.is_none() {
            if let Some(sid) = &short_id {
                id = self
                    .resolve_redirect(&format!("{REDDIT_BASE}/video/{sid}"))
                    .await;
            }
        }
        if id.is_none() {
            if let (Some(sub_name), Some(sid)) = (&sub, &share_id) {
                id = self
                    .resolve_redirect(&format!("{REDDIT_BASE}/r/{sub_name}/s/{sid}"))
                    .await;
            }
        }

        let post_id = id?;
        let json_url = format!("{REDDIT_BASE}/comments/{post_id}.json");
        let resp = self
            .client
            .get(&json_url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }

        let body: serde_json::Value = resp.json().await.ok()?;
        let listing = body.as_array()?.first()?;
        let post = listing
            .pointer("/data/children/0/data")?;

        let title = post["title"].as_str().unwrap_or("Reddit Video");
        let author = post["author"]
            .as_str()
            .map(|a| format!("u/{a}"))
            .unwrap_or_else(|| "Reddit".into());
        let thumbnail = post["thumbnail"]
            .as_str()
            .or_else(|| {
                post.pointer("/preview/images/0/source/url")
                    .and_then(|v| v.as_str())
            })
            .map(|s| s.to_string());

        // Skip GIFs
        if post["url"].as_str().map(|u| u.ends_with(".gif")).unwrap_or(false) {
            return None;
        }

        let reddit_video = post.pointer("/secure_media/reddit_video")?;
        let fallback_url = reddit_video["fallback_url"].as_str()?;
        let video_url = fallback_url.split('?').next().unwrap_or(fallback_url);
        let duration = reddit_video["duration"]
            .as_u64()
            .unwrap_or(0) * 1000;

        let audio_url = self.find_audio_url(video_url).await;

        let identifier = sub
            .as_ref()
            .map(|s| format!("{}_{post_id}", s.to_lowercase()))
            .unwrap_or_else(|| post_id.clone());

        let track_url = if audio_url.is_some() {
            // Tunnel - return audio URL
            audio_url.clone().unwrap_or_default()
        } else {
            video_url.to_string()
        };

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier,
                is_seekable: true,
                author,
                length: duration as i64,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: Some(url.to_string()),
                artwork_url: thumbnail,
                isrc: None,
                source_name: "reddit".into(),
                chapters: None,
            },
            plugin_info: serde_json::json!({
                "directUrl": track_url,
                "hasAudio": audio_url.is_some(),
            }),
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        Some(track)
    }

    async fn get_audio_url(&self, uri: &str) -> Option<String> {
        let track = self.get_reddit_track(uri).await?;
        let direct = track.plugin_info.get("directUrl")?.as_str()?.to_string();
        Some(direct)
    }
}

#[async_trait]
impl SourceProvider for RedditSource {
    fn name(&self) -> &'static str {
        "reddit"
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
        match self.get_reddit_track(query).await {
            Some(track) => Ok(SourceResult::Track(track)),
            None => Ok(SourceResult::Empty),
        }
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

        if let Some(url) = self.get_audio_url(uri).await {
            let has_audio = url.contains("audio") || url.contains("_AUDIO_");
            Ok(TrackUrlResult {
                url: Some(url),
                protocol: Some("https".into()),
                format: serde_json::json!(if has_audio { "mp3" } else { "mp4" }),
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
                exception: Some("Failed to resolve Reddit track URL".into()),
            })
        }
    }
}
