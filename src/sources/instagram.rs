use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::sync::Mutex;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36";

struct IgConfig {
    csrf_token: Option<String>,
    ig_app_id: Option<String>,
    fb_lsd: Option<String>,
    doc_id_post: String,
}

pub struct InstagramSource {
    client: Client,
    config: Mutex<IgConfig>,
}

impl InstagramSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Mutex::new(IgConfig {
                csrf_token: None,
                ig_app_id: None,
                fb_lsd: None,
                doc_id_post: "10015901848480474".into(),
            }),
        }
    }

    async fn ensure_config(&self) -> bool {
        {
            let cfg = self.config.lock().unwrap();
            if cfg.csrf_token.is_some() && cfg.ig_app_id.is_some() && cfg.fb_lsd.is_some() {
                return true;
            }
        }

        let resp = self
            .client
            .get("https://www.instagram.com/")
            .header("User-Agent", USER_AGENT)
            .send()
            .await;

        let html = match resp {
            Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
            _ => return false,
        };

        let csrf_re = Regex::new(r#""csrf_token":"(.*?)""#).ok();
        let appid_re = Regex::new(r#""appId":"(.*?)""#).ok();
        let lsd_re = Regex::new(r#""LSD",\[\],\{"token":"(.*?)"\},"#).ok();
        let doc_re = Regex::new(r#""PostPage",\[\],"(\d+)","#).ok();

        let csrf = csrf_re.and_then(|r| r.captures(&html)).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        let appid = appid_re.and_then(|r| r.captures(&html)).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        let lsd = lsd_re.and_then(|r| r.captures(&html)).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
        let doc = doc_re.and_then(|r| r.captures(&html)).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());

        let mut cfg = self.config.lock().unwrap();
        cfg.csrf_token = csrf;
        cfg.ig_app_id = appid;
        cfg.fb_lsd = lsd;
        if let Some(d) = doc {
            cfg.doc_id_post = d;
        }

        cfg.csrf_token.is_some() && cfg.ig_app_id.is_some() && cfg.fb_lsd.is_some()
    }

    fn with_config<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&IgConfig) -> R,
    {
        let cfg = self.config.lock().unwrap();
        f(&cfg)
    }

    fn audio_pattern() -> Regex {
        Regex::new(r"^https?://(?:www\.)?instagram\.com/reels/audio/(\d+)").unwrap()
    }

    fn post_pattern() -> Regex {
        Regex::new(r"^https?://(?:www\.)?instagram\.com/p/([\w-]+)").unwrap()
    }

    fn reel_pattern() -> Regex {
        Regex::new(r"^https?://(?:www\.)?instagram\.com/(?:reels?|reel)/([\w-]+)").unwrap()
    }

    fn extract_info(query: &str) -> Option<(String, &'static str, Option<&'static str>)> {
        if let Some(caps) = Self::audio_pattern().captures(query) {
            return Some((caps[1].to_string(), "audio", None));
        }
        if let Some(caps) = Self::post_pattern().captures(query) {
            return Some((caps[1].to_string(), "post", Some("p")));
        }
        if let Some(caps) = Self::reel_pattern().captures(query) {
            return Some((caps[1].to_string(), "post", Some("reel")));
        }
        None
    }

    fn extract_meta_content(html: &str, property: &str) -> Option<String> {
        let re1 = Regex::new(&format!(
            r#"<meta[^>]+property=["']{}["'][^>]+content=["']([^"']+)["']"#,
            regex::escape(property)
        ))
        .ok()?;
        if let Some(caps) = re1.captures(html) {
            return Some(Self::decode_html(caps[1].to_string()));
        }
        let re2 = Regex::new(&format!(
            r#"<meta[^>]+content=["']([^"']+)["'][^>]+property=["']{}["']"#,
            regex::escape(property)
        ))
        .ok()?;
        if let Some(caps) = re2.captures(html) {
            return Some(Self::decode_html(caps[1].to_string()));
        }
        None
    }

    fn decode_html(text: String) -> String {
        text.replace("&amp;", "&")
            .replace("&#39;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .trim()
            .to_string()
    }

    fn parse_audio_og_metadata(og_title: Option<&str>, og_desc: Option<&str>) -> (String, String) {
        let title = og_title.unwrap_or("");
        let desc = og_desc.unwrap_or("");
        let normalized = title.replace(" on Instagram", "").trim().to_string();

        if normalized.contains(" | ") {
            let parts: Vec<&str> = normalized.splitn(2, " | ").collect();
            return (parts[0].trim().to_string(), parts[1].trim().to_string());
        }

        if let Some(caps) =
            Regex::new(r"Listen to (.+?) on Instagram and watch reels using (.+?) audio")
                .ok()
                .and_then(|re| re.captures(desc))
        {
            return (
                caps[1].trim().to_string(),
                caps[2].trim().to_string(),
            );
        }

        ("User Unknown".into(), normalized)
    }

    async fn fetch_audio_og(&self, audio_id: &str) -> Option<TrackInfo> {
        let url = format!("https://www.instagram.com/reels/audio/{}/", audio_id);
        let resp = self
            .client
            .get(&url)
            .header("User-Agent", "facebookexternalhit/1.1")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        let html = resp.text().await.ok()?;
        let og_title = Self::extract_meta_content(&html, "og:title");
        let og_desc = Self::extract_meta_content(&html, "og:description");
        let og_image = Self::extract_meta_content(&html, "og:image");

        let (author, title) = Self::parse_audio_og_metadata(og_title.as_deref(), og_desc.as_deref());

        Some(TrackInfo {
            identifier: audio_id.to_string(),
            is_seekable: true,
            author: author.clone(),
            length: 0,
            is_stream: false,
            position: 0,
            title,
            uri: Some(url),
            artwork_url: og_image,
            isrc: None,
            source_name: "instagram".into(),
            chapters: None,
        })
    }

    fn encode_post_request(shortcode: &str, lsd: &str, doc_id: &str) -> String {
        let variables = serde_json::json!({
            "shortcode": shortcode,
            "fetch_comment_count": null,
            "fetch_related_profile_media_count": null,
            "parent_comment_count": null,
            "child_comment_count": null,
            "fetch_like_count": null,
            "fetch_tagged_user_count": null,
            "fetch_preview_comment_count": null,
            "has_threaded_comments": false,
            "hoisted_comment_id": null,
            "hoisted_reply_id": null,
        });

        let params = [
            ("av", "0"),
            ("__user", "0"),
            ("__a", "1"),
            ("__req", "3"),
            ("dpr", "1"),
            ("__ccg", "UNKNOWN"),
            ("lsd", lsd),
            ("jazoest", "2957"),
            ("doc_id", doc_id),
            ("variables", &variables.to_string()),
            ("fb_api_req_friendly_name", "PolarisPostActionLoadPostQueryQuery"),
            ("fb_api_caller_class", "RelayModern"),
        ];

        let mut encoded = String::new();
        for (i, (k, v)) in params.iter().enumerate() {
            if i > 0 {
                encoded.push('&');
            }
            encoded.push_str(&urlencoding(k));
            encoded.push('=');
            encoded.push_str(&urlencoding(v));
        }
        encoded
    }

    async fn fetch_from_graphql(
        &self,
        shortcode: &str,
        path_segment: &str,
    ) -> Option<(TrackInfo, String)> {
        let (csrf, app_id, lsd, doc_id) = {
            let cfg = self.config.lock().ok()?;
            (cfg.csrf_token.clone()?, cfg.ig_app_id.clone()?, cfg.fb_lsd.clone()?, cfg.doc_id_post.clone())
        };

        let body = Self::encode_post_request(shortcode, &lsd, &doc_id);

        let resp = self
            .client
            .post("https://www.instagram.com/api/graphql")
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("X-FB-Friendly-Name", "PolarisPostActionLoadPostQueryQuery")
            .header("X-CSRFToken", csrf)
            .header("X-IG-App-ID", app_id)
            .header("X-FB-LSD", lsd)
            .header("X-ASBD-ID", "129477")
            .header("Sec-Fetch-Site", "same-origin")
            .header("User-Agent", USER_AGENT)
            .header("Origin", "https://www.instagram.com")
            .header("Referer", &format!("https://www.instagram.com/{}/{}", path_segment, shortcode))
            .body(body)
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        let text = resp.text().await.ok()?;
        let data: Value = serde_json::from_str(&text).ok()?;
        let media = data.get("data")?.get("xdt_shortcode_media")?;

        let video_url = if media.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false) {
            media.get("video_url")
        } else if media.get("__typename").and_then(|t| t.as_str()) == Some("XDTGraphSidecar") {
            media.get("edge_sidecar_to_children")
                .and_then(|c| c.get("edges"))
                .and_then(|e| e.as_array())
                .and_then(|arr| {
                    arr.iter().find(|edge| {
                        edge.get("node")
                            .and_then(|n| n.get("is_video"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    })
                })
                .and_then(|edge| edge.get("node")?.get("video_url"))
        } else {
            None
        }?;

        let video_url_str = video_url.as_str()?.to_string();

        let caption = media
            .get("edge_media_to_caption")
            .and_then(|c| c.get("edges"))
            .and_then(|e| e.as_array())
            .and_then(|arr| arr.first())
            .and_then(|edge| edge.get("node")?.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("Instagram Video");

        let owner = media.get("owner").and_then(|o| o.get("username")).and_then(|u| u.as_str()).unwrap_or("User Unknown");

        let duration = if let Some(v) = media.get("video_duration").and_then(|d| d.as_f64()) {
            (v * 1000.0) as i64
        } else {
            0
        };

        let thumbnail = media.get("display_url").and_then(|u| u.as_str()).map(|s| s.to_string());

        let track_info = TrackInfo {
            identifier: shortcode.to_string(),
            is_seekable: true,
            author: owner.to_string(),
            length: duration,
            is_stream: false,
            position: 0,
            title: caption.to_string(),
            uri: Some(format!("https://www.instagram.com/{}/{}", path_segment, shortcode)),
            artwork_url: thumbnail,
            isrc: None,
            source_name: "instagram".into(),
            chapters: None,
        };

        Some((track_info, video_url_str))
    }
}

#[async_trait]
impl SourceProvider for InstagramSource {
    fn name(&self) -> &'static str {
        "instagram"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["ig"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &[]
    }

    async fn search(&self, _query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        Ok(SourceResult::Empty)
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let (content_id, content_type, path_segment) = match Self::extract_info(query) {
            Some(info) => info,
            None => return Ok(SourceResult::Empty),
        };

        if content_type == "audio" {
            // Try OG metadata first
            if let Some(info) = self.fetch_audio_og(&content_id).await {
                let mut track = TrackData {
                    encoded: None,
                    info,
                    plugin_info: serde_json::json!({}),
                    user_data: serde_json::json!({}),
                    details: Vec::new(),
                    message_flags: 0,
                };
                track.encoded = Some(encode_track(&track));
                return Ok(SourceResult::Track(track));
            }
            return Ok(SourceResult::Empty);
        }

        // Post/reel - needs config
        self.ensure_config().await;
        let seg = path_segment.unwrap_or("p");
        if let Some((info, _)) = self.fetch_from_graphql(&content_id, seg).await {
            let mut track = TrackData {
                encoded: None,
                info,
                plugin_info: serde_json::json!({}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            track.encoded = Some(encode_track(&track));
            return Ok(SourceResult::Track(track));
        }

        Ok(SourceResult::Empty)
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

        let (content_id, content_type, path_segment) = match Self::extract_info(uri) {
            Some(info) => info,
            None => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some("Could not parse Instagram URI".into()),
                })
            }
        };

        if content_type == "audio" {
            // Audio tracks don't have a direct URL from OG metadata
            // Return the audio page URL - the client will need to handle it
            return Ok(TrackUrlResult {
                url: Some(uri.clone()),
                protocol: Some("https".into()),
                format: serde_json::json!("mp4"),
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: Some("Instagram audio requires browser playback".into()),
            });
        }

        let seg = path_segment.unwrap_or("p");
        if let Some((_, video_url)) = self.fetch_from_graphql(&content_id, seg).await {
            Ok(TrackUrlResult {
                url: Some(video_url),
                protocol: Some("https".into()),
                format: serde_json::json!("mp4"),
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
                exception: Some("Failed to resolve Instagram track URL".into()),
            })
        }
    }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
