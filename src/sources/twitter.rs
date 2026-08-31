use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::sync::Mutex;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const BEARER_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TOKEN_TTL: u64 = 60 * 60 * 3;

pub struct TwitterSource {
    client: Client,
    guest_token: Mutex<Option<(String, u64)>>,
}

impl TwitterSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            guest_token: Mutex::new(None),
        }
    }

    fn tweet_pattern() -> Regex {
        Regex::new(r"https?://(?:(?:www|m(?:obile)?)\.)?(?:twitter|x)\.com/(?:[^/]+)/status/(\d+)")
            .unwrap()
    }

    fn extract_tweet_id(query: &str) -> Option<String> {
        let re = Self::tweet_pattern();
        re.captures(query).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
    }

    fn generate_syndication_token(identifier: &str) -> String {
        let num: f64 = identifier.parse().unwrap_or(0.0);
        let val = (num / 1e15) * std::f64::consts::PI;
        let result = format!("{:.20}", val);
        result.replace(".", "").replace("0", "")
    }

    async fn refresh_guest_token(&self) -> Option<String> {
        let resp = self
            .client
            .post("https://api.twitter.com/1.1/guest/activate.json")
            .header("Authorization", format!("Bearer {}", BEARER_TOKEN))
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        let data: Value = resp.json().await.ok()?;
        data.get("guest_token")?.as_str().map(|s| s.to_string())
    }

    async fn ensure_token(&self) -> Option<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        {
            let guard = self.guest_token.lock().ok()?;
            if let Some((ref token, expiry)) = *guard {
                if now < expiry {
                    return Some(token.clone());
                }
            }
        }

        let token = self.refresh_guest_token().await?;
        if let Ok(mut guard) = self.guest_token.lock() {
            *guard = Some((token.clone(), now + TOKEN_TTL));
        }
        Some(token)
    }

    fn json_to_encoded(val: &Value) -> String {
        urlencoding(&val.to_string())
    }

    fn find_best_variant(val: &Value) -> Option<(String, bool)> {
        let variants = val.get("variants")?.as_array()?;
        let mut best_mp4: Option<(String, i64)> = None;
        let mut hls_url: Option<String> = None;

        for v in variants {
            let content_type = v.get("content_type").and_then(|c| c.as_str()).unwrap_or("");
            let url = v.get("url").and_then(|u| u.as_str())?;

            if content_type == "video/mp4" {
                let bitrate = v.get("bitrate").and_then(|b| b.as_i64()).unwrap_or(0);
                let is_better = match best_mp4 {
                    Some((_, existing_bitrate)) => bitrate > existing_bitrate,
                    None => true,
                };
                if is_better {
                    best_mp4 = Some((url.to_string(), bitrate));
                }
            } else if content_type == "application/x-mpegURL" || url.contains(".m3u8") {
                if hls_url.is_none() {
                    hls_url = Some(url.to_string());
                }
            }
        }

        if let Some((url, _)) = best_mp4 {
            Some((url, false))
        } else if let Some(url) = hls_url {
            Some((url, true))
        } else {
            None
        }
    }

    fn extract_track_from_graphql(&self, data: &Value, identifier: &str, url: &str) -> Option<(TrackInfo, String, bool)> {
        let result = Self::get_nested_value(data, &["data", "tweetResult", "result"])?;
        let result = Self::unwrap_tweet(result);
        Self::extract_media(result, identifier, url)
    }

    fn unwrap_tweet<'a>(val: &'a Value) -> &'a Value {
        if val.get("__typename").and_then(|t| t.as_str()) == Some("TweetWithVisibilityResults") {
            val.get("tweet").unwrap_or(val)
        } else {
            val
        }
    }

    fn extract_media(val: &Value, identifier: &str, url: &str) -> Option<(TrackInfo, String, bool)> {
        let legacy = val.get("legacy")?;
        let full_text = legacy.get("full_text").and_then(|t| t.as_str()).unwrap_or("Twitter Content");
        let title = full_text.split("https://t.co").next().unwrap_or("Twitter Content").trim().to_string();
        let title = if title.is_empty() { "Twitter Content".to_string() } else { title };

        let extended = val.get("extended_entities").or_else(|| legacy.get("extended_entities"));
        let media = extended
            .and_then(|e| e.get("media"))
            .and_then(|m| m.as_array())
            .and_then(|arr| {
                arr.iter().find(|item| {
                    let t = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    t == "video" || t == "animated_gif"
                })
            })?;

        let video_info = media.get("video_info")?;
        let (direct_url, is_hls) = Self::find_best_variant(video_info)?;
        let duration = video_info.get("duration_millis").and_then(|d| d.as_i64()).unwrap_or(0);

        let author = Self::get_nested_string(val, &["core", "user_results", "result", "legacy", "name"])
            .unwrap_or_else(|| "Twitter User".to_string());

        let artwork = media.get("media_url_https").and_then(|m| m.as_str()).map(|s| s.to_string());

        let track_info = TrackInfo {
            identifier: identifier.to_string(),
            is_seekable: true,
            author,
            length: duration,
            is_stream: is_hls,
            position: 0,
            title,
            uri: Some(url.to_string()),
            artwork_url: artwork,
            isrc: None,
            source_name: "twitter".into(),
            chapters: None,
        };

        Some((track_info, direct_url, is_hls))
    }

    fn extract_track_from_syndication(&self, data: &Value, identifier: &str, url: &str) -> Option<(TrackInfo, String, bool)> {
        let media = data.get("video").or_else(|| {
            data.get("mediaDetails")
                .and_then(|m| m.as_array())
                .and_then(|arr| arr.first())
        })?;

        let (direct_url, is_hls) = Self::find_best_variant(media)?;
        let duration = media.get("durationMs").and_then(|d| d.as_i64()).unwrap_or(0);

        let author = data.get("user")
            .and_then(|u| u.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Twitter User")
            .to_string();

        let text = data.get("text").and_then(|t| t.as_str()).unwrap_or("Twitter Content");
        let title = text.split("https://t.co").next().unwrap_or("Twitter Content").trim().to_string();
        let title = if title.is_empty() { "Twitter Content".to_string() } else { title };

        let artwork = media.get("poster").and_then(|p| p.as_str()).map(|s| s.to_string());

        let track_info = TrackInfo {
            identifier: identifier.to_string(),
            is_seekable: true,
            author,
            length: duration,
            is_stream: is_hls,
            position: 0,
            title,
            uri: Some(url.to_string()),
            artwork_url: artwork,
            isrc: None,
            source_name: "twitter".into(),
            chapters: None,
        };

        Some((track_info, direct_url, is_hls))
    }

    fn get_nested_string<'a>(val: &'a Value, path: &[&str]) -> Option<String> {
        let mut current = val;
        for key in path {
            current = current.get(*key)?;
        }
        current.as_str().map(|s| s.to_string())
    }

    fn get_nested_value<'a>(val: &'a Value, path: &[&str]) -> Option<&'a Value> {
        let mut current = val;
        for key in path {
            current = current.get(*key)?;
        }
        Some(current)
    }

    async fn resolve_via_graphql(&self, identifier: &str, guest_token: &str, url: &str) -> Option<(TrackInfo, String, bool)> {
        let variables = serde_json::json!({
            "tweetId": identifier,
            "withCommunity": false,
            "includePromotedContent": false,
            "withVoice": true
        });
        let features = serde_json::json!({
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true
        });

        let api_url = format!(
            "https://twitter.com/i/api/graphql/2ICDjqPd81tulZcYrtpTuQ/TweetResultByRestId?variables={}&features={}",
            Self::json_to_encoded(&variables),
            Self::json_to_encoded(&features)
        );

        let resp = self
            .client
            .get(&api_url)
            .header("Authorization", format!("Bearer {}", BEARER_TOKEN))
            .header("x-guest-token", guest_token)
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-client-language", "en")
            .header("User-Agent", USER_AGENT)
            .header("Referer", "https://twitter.com/")
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        let data: Value = resp.json().await.ok()?;
        self.extract_track_from_graphql(&data, identifier, url)
    }

    async fn resolve_via_syndication(&self, identifier: &str, url: &str) -> Option<(TrackInfo, String, bool)> {
        let tok = Self::generate_syndication_token(identifier);
        let api_url = format!(
            "https://cdn.syndication.twimg.com/tweet-result?id={}&token={}&lang=en",
            identifier, tok
        );

        let resp = self
            .client
            .get(&api_url)
            .header("User-Agent", "Googlebot")
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        let data: Value = resp.json().await.ok()?;
        self.extract_track_from_syndication(&data, identifier, url)
    }
}

#[async_trait]
impl SourceProvider for TwitterSource {
    fn name(&self) -> &'static str {
        "twitter"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["x"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["twsearch"]
    }

    async fn search(&self, _query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        Ok(SourceResult::Empty)
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let identifier = match Self::extract_tweet_id(query) {
            Some(id) => id,
            None => return Ok(SourceResult::Empty),
        };

        // Try GraphQL API first
        let token = self.ensure_token().await;
        if let Some(ref token) = token {
            if let Some((info, _, _)) = self.resolve_via_graphql(&identifier, token, query).await {
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
        }

        // Fallback to syndication API
        if let Some((info, _, _)) = self.resolve_via_syndication(&identifier, query).await {
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

        let identifier = match Self::extract_tweet_id(uri) {
            Some(id) => id,
            None => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::json!({}),
                    new_track: None,
                    additional_data: serde_json::json!({}),
                    exception: Some("No tweet ID found in URI".into()),
                })
            }
        };

        // Try syndication API first (no auth needed)
        if let Some((_, direct_url, is_hls)) = self.resolve_via_syndication(&identifier, uri).await {
            return Ok(TrackUrlResult {
                url: Some(direct_url),
                protocol: Some(if is_hls { "hls".to_string() } else { "https".to_string() }),
                format: serde_json::json!(if is_hls { "m3u8" } else { "mp4" }),
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: None,
            });
        }

        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: serde_json::json!({}),
            new_track: None,
            additional_data: serde_json::json!({}),
            exception: Some("Failed to resolve Twitter track URL".into()),
        })
    }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
