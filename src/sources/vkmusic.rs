use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const API_BASE: &str = "https://api.vk.com/method/";
const API_VERSION: &str = "5.131";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:146.0) Gecko/20100101 Firefox/146.0";
const MOBILE_UA: &str = "KateMobileAndroid/56 lite-460 (Android 4.4.2; SDK 19; x86; unknown Android SDK built for x86; en)";

const B64_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMN0PQRSTUVWXYZO123456789+/=";

struct VkAuth {
    access_token: String,
    user_id: i64,
    cookie: String,
}

pub struct VKMusicSource {
    client: Client,
    user_token: Option<String>,
    user_cookie: Option<String>,
    auth: std::sync::Mutex<Option<VkAuth>>,
}

impl VKMusicSource {
    pub fn new(user_token: Option<String>, user_cookie: Option<String>) -> Self {
        Self {
            client: Client::new(),
            user_token,
            user_cookie,
            auth: std::sync::Mutex::new(None),
        }
    }

    fn get_auth(&self) -> Option<VkAuth> {
        self.auth.lock().unwrap().clone()
    }

    fn set_auth(&self, a: VkAuth) {
        *self.auth.lock().unwrap() = Some(a);
    }

    fn pattern() -> Vec<Regex> {
        vec![
            Regex::new(r"vk\.(?:com|ru)/.*?[?&]z=audio_playlist(-?\d+)_(\d+)(?:(?:%2F|_|/|(?:\?|&)access_hash=)([a-z0-9]+))?").unwrap(),
            Regex::new(r"vk\.(?:com|ru)/(?:music/(?:playlist|album)/)(-?\d+)_(\d+)(?:(?:%2F|_|/|(?:\?|&)access_hash=)([a-z0-9]+))?").unwrap(),
            Regex::new(r"vk\.(?:com|ru)/audio(-?\d+)_(\d+)(?:(?:%2F|_|/)([a-z0-9]+))?").unwrap(),
            Regex::new(r"vk\.(?:com|ru)/audios(-?\d+)").unwrap(),
        ]
    }

    async fn ensure_auth(&self) -> Option<VkAuth> {
        if let Some(a) = self.get_auth() {
            return Some(a);
        }

        let token = self.user_token.clone();
        let cookie = self.user_cookie.clone().unwrap_or_default();

        if let Some(ref t) = token {
            let a = VkAuth {
                access_token: t.clone(),
                user_id: 0,
                cookie: cookie.clone(),
            };
            self.set_auth(VkAuth {
                access_token: t.clone(),
                user_id: 0,
                cookie: cookie.clone(),
            });
            return Some(a);
        }

        if !cookie.is_empty() {
            if let Some(a) = self.refresh_token(&cookie).await {
                return Some(a);
            }
        }

        None
    }

    async fn refresh_token(&self, cookie: &str) -> Option<VkAuth> {
        let resp = self
            .client
            .post("https://login.vk.ru/?act=web_token")
            .header("User-Agent", USER_AGENT)
            .header("Referer", "https://vk.ru/")
            .header("Origin", "https://vk.ru")
            .header("Cookie", cookie)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body("version=1&app_id=6287487")
            .send()
            .await
            .ok()?;

        let body: Value = resp.json().await.ok()?;
        let data = body.get("data")?;
        let access_token = data.get("access_token")?.as_str()?.to_string();
        let user_id = data.get("user_id")?.as_i64().unwrap_or(0);

        let a = VkAuth {
            access_token,
            user_id,
            cookie: cookie.to_string(),
        };
        self.set_auth(a.clone());
        Some(a)
    }

    async fn api_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<(&str, &str)>,
    ) -> Option<T> {
        let auth = self.ensure_auth().await?;

        let mut all_params = params;
        all_params.push(("access_token", &auth.access_token));
        all_params.push(("v", API_VERSION));

        let url = reqwest::Url::parse_with_params(&format!("{}{}", API_BASE, method), &all_params).ok()?;

        let resp = self
            .client
            .get(url)
            .header("User-Agent", MOBILE_UA)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }

        let body: Value = resp.json().await.ok()?;

        if body.get("error").is_some() {
            if cookie_auth_possible(&body) && !auth.cookie.is_empty() {
                self.refresh_token(&auth.cookie).await;
                return Box::pin(self.api_request(method, all_params)).await;
            }
            return None;
        }

        body.get("response").and_then(|r| {
            serde_json::from_value(r.clone()).ok()
        })
    }

    fn b64_decode(enc: &str) -> String {
        let mut dec = String::new();
        let mut e = 0u32;
        let mut n = 0u32;
        for byte in enc.bytes() {
            let r = B64_CHARS.iter().position(|&c| c == byte);
            let r = match r {
                Some(pos) => pos as u32,
                None => continue,
            };
            e = if n % 4 != 0 { 64 * e + r } else { r };
            n += 1;
            if n % 4 != 0 {
                dec.push(char::from((0xff & (e >> ((-2 * n as i32) & 6) as u32)) as u8));
            }
        }
        dec
    }

    fn unmask_url(mask_url: &str, vk_id: i64) -> Option<String> {
        if !mask_url.contains("audio_api_unavailable") {
            return Some(mask_url.to_string());
        }

        let after_extra = mask_url.split("?extra=").nth(1)?;
        let parts: Vec<&str> = after_extra.split('#').collect();
        if parts.len() < 2 {
            return Some(mask_url.to_string());
        }

        let p1 = Self::b64_decode(parts[1]);
        let split1: Vec<&str> = p1.split(char::from(11u8)).collect();
        let s1 = split1.get(1)?;

        let mut mask_chars: Vec<char> = Self::b64_decode(parts[0]).chars().collect();
        let index_initial = s1.parse::<i64>().ok()? ^ vk_id;
        let url_len = mask_chars.len();
        let mut indexes = vec![0usize; url_len];
        let mut index = index_initial;

        for n in (0..url_len).rev() {
            index = ((url_len as i64 * (n as i64 + 1)) ^ (index + n as i64)) % url_len as i64;
            indexes[n] = index as usize;
        }

        for n in 1..url_len {
            let c = mask_chars[n];
            let idx = indexes[url_len - 1 - n];
            if idx < mask_chars.len() {
                mask_chars[n] = mask_chars[idx];
                mask_chars[idx] = c;
            }
        }

        Some(mask_chars.iter().collect())
    }

    fn build_track(item: &Value) -> Option<TrackData> {
        let id = format!("{}_{}", item.get("owner_id")?.as_i64()?, item.get("id")?.as_i64()?);
        let artist = item.get("artist").and_then(|v| v.as_str()).unwrap_or("Unknown Artist");
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown Title");
        let duration = item.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);

        let artwork_url = item
            .get("album")
            .and_then(|a| a.get("thumb").or_else(|| {
                a.get("images").and_then(|im| im.as_array()).and_then(|arr| arr.first())
            }))
            .and_then(|t| {
                t.get("photo_1200")
                    .or_else(|| t.get("photo_600"))
                    .or_else(|| t.get("photo_300"))
                    .or_else(|| t.get("url"))
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            });

        let access_key = item.get("access_key").and_then(|v| v.as_str()).map(|s| s.to_string());
        let isrc = item
            .get("external_ids")
            .and_then(|e| e.get("isrc"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let info = TrackInfo {
            identifier: id.clone(),
            is_seekable: true,
            author: artist.to_string(),
            length: duration * 1000,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri: Some(format!("https://vk.com/audio{}_", id.clone())),
            artwork_url,
            isrc,
            source_name: "vkmusic".into(),
            chapters: None,
        };

        let mut plugin_info = serde_json::Map::new();
        if let Some(ak) = access_key {
            plugin_info.insert("access_key".into(), Value::String(ak));
        }

        let encoded = encode_track(&TrackData {
            encoded: None,
            info: info.clone(),
            plugin_info: Value::Object(plugin_info.clone()),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        });

        Some(TrackData {
            encoded: Some(encoded),
            info,
            plugin_info: Value::Object(plugin_info),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        })
    }

    fn parse_meta(data: &Value) -> Option<TrackData> {
        let arr = data.as_array()?;
        if arr.len() < 6 {
            return None;
        }
        let id = format!("{}_{}", arr.get(1)?.as_i64()?, arr.first()?.as_i64()?);
        let _raw_url = arr.get(2)?.as_str().unwrap_or("");
        let title = arr.get(3).and_then(|v| v.as_str()).unwrap_or("Unknown Title");
        let author = arr.get(4).and_then(|v| v.as_str()).unwrap_or("Unknown Artist");
        let duration = arr.get(5).and_then(|v| v.as_i64()).unwrap_or(0);

        let artwork_raw = arr.get(14).and_then(|v| v.as_str());
        let artwork_url = artwork_raw.and_then(|s| s.split(',').next().map(|s| s.to_string()));

        let info = TrackInfo {
            identifier: id.clone(),
            is_seekable: true,
            author: author.to_string(),
            length: duration * 1000,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri: Some(format!("https://vk.com/audio{}", id.clone())),
            artwork_url,
            isrc: None,
            source_name: "vkmusic".into(),
            chapters: None,
        };

        let encoded = encode_track(&TrackData {
            encoded: None,
            info: info.clone(),
            plugin_info: Value::Object(serde_json::Map::new()),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        });

        Some(TrackData {
            encoded: Some(encoded),
            info,
            plugin_info: Value::Object(serde_json::Map::new()),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        })
    }

    async fn scrape_playlist(&self, url: &str) -> anyhow::Result<SourceResult> {
        let cookie = self.get_auth().map(|a| a.cookie).unwrap_or_default();
        let resp = self
            .client
            .get(url)
            .header("User-Agent", USER_AGENT)
            .header("Cookie", cookie)
            .send()
            .await?;

        let body = resp.text().await?;
        let re = Regex::new(r#"data-audio="([^"]+)""#)?;
        let tracks: Vec<TrackData> = re.captures_iter(&body)
            .filter_map(|cap| cap.get(1))
            .filter_map(|m| {
                let raw = m.as_str().replace("&quot;", "\"");
                serde_json::from_str::<Value>(&raw).ok()
                    .and_then(|v| Self::parse_meta(&v))
            })
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: "VK Scraped Playlist".into(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "playlist"}),
                tracks,
            },
        })
    }

    async fn scrape_track(&self, url: &str) -> anyhow::Result<Option<TrackData>> {
        let cookie = self.get_auth().map(|a| a.cookie).unwrap_or_default();
        let resp = self
            .client
            .get(url)
            .header("User-Agent", USER_AGENT)
            .header("Cookie", cookie)
            .send()
            .await?;

        let body = resp.text().await?;
        let re = Regex::new(r#"data-audio="([^"]+)""#)?;
        if let Some(cap) = re.captures(&body) {
            if let Some(m) = cap.get(1) {
                let raw = m.as_str().replace("&quot;", "\"");
                if let Ok(data) = serde_json::from_str::<Value>(&raw) {
                    return Ok(Self::parse_meta(&data));
                }
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl SourceProvider for VKMusicSource {
    fn name(&self) -> &'static str {
        "vkmusic"
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["vksearch"]
    }

    async fn search(&self, query: &str, search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        if let Some(st) = search_type {
            if st == "vkrec" {
                return self.recommendations_inner(query).await;
            }
        }

        match self.api_request::<Value>("audio.search", vec![("q", query), ("count", "10"), ("extended", "1")]).await {
            Some(data) => {
                let items = data.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
                let tracks: Vec<TrackData> = items.iter().filter_map(Self::build_track).collect();
                if tracks.is_empty() {
                    Ok(SourceResult::Empty)
                } else {
                    Ok(SourceResult::Search { data: tracks })
                }
            }
            None => Ok(SourceResult::error("VK authentication required.".into())),
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let patterns = Self::pattern();

        // Playlist match (patterns 0, 1)
        for pi in 0..2 {
            if let Some(caps) = patterns[pi].captures(query) {
                let owner = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let hash = caps.get(3).map(|m| m.as_str());
                if !owner.is_empty() && !id.is_empty() {
                    return self.resolve_playlist_inner(owner, Some(id), hash).await;
                }
            }
        }

        // Single track (pattern 2)
        if let Some(caps) = patterns[2].captures(query) {
            return self.resolve_track_inner(query, &caps).await;
        }

        // User audios (pattern 3)
        if let Some(caps) = patterns[3].captures(query) {
            if let Some(id) = caps.get(1).map(|m| m.as_str()) {
                return self.resolve_playlist_inner(id, None, None).await;
            }
        }

        Ok(SourceResult::Empty)
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let id = &track.identifier;

        // Try API
        let _access_key: Option<String> = track.uri.as_ref().and_then(|_| None); // could extract from pluginInfo
        let audios = id.clone();

        if let Some(items) = self
            .api_request::<Vec<Value>>("audio.getById", vec![("audios", &audios)])
            .await
        {
            if let Some(item) = items.first() {
                if let Some(raw_url) = item.get("url").and_then(|v| v.as_str()) {
                    let auth = self.get_auth();
                    let user_id = auth.as_ref().map(|a| a.user_id).unwrap_or(0);
                    let url = Self::unmask_url(raw_url, user_id).unwrap_or_else(|| raw_url.to_string());

                    if url.starts_with("http") || url.contains(".m3u8") {
                        let protocol = if url.contains(".m3u8") { "hls" } else { "https" };
                        let format = if url.contains(".m3u8") { "mpegts" } else { "mp3" };
                        return Ok(TrackUrlResult {
                            url: Some(url),
                            protocol: Some(protocol.into()),
                            format: serde_json::json!(format),
                            new_track: None,
                            additional_data: serde_json::Value::Object(serde_json::Map::new()),
                            exception: None,
                        });
                    }
                }
            }
        }

        // Fallback: search for the track
        let search_query = format!("{} {}", track.author, track.title);
        if let Some(data) = self
            .api_request::<Value>("audio.search", vec![("q", &search_query), ("count", "10")])
            .await
        {
            if let Some(items) = data.get("items").and_then(|i| i.as_array()) {
                for item in items {
                    let item_id = format!("{}_{}", item.get("owner_id").and_then(|v| v.as_i64()).unwrap_or(0), item.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                    if item_id == *id || true {
                        if let Some(raw_url) = item.get("url").and_then(|v| v.as_str()) {
                            let auth = self.get_auth();
                            let user_id = auth.as_ref().map(|a| a.user_id).unwrap_or(0);
                            let url = Self::unmask_url(raw_url, user_id).unwrap_or_else(|| raw_url.to_string());

                            if url.starts_with("http") || url.contains(".m3u8") {
                                let protocol = if url.contains(".m3u8") { "hls" } else { "https" };
                                let format = if url.contains(".m3u8") { "mpegts" } else { "mp3" };
                                return Ok(TrackUrlResult {
                                    url: Some(url),
                                    protocol: Some(protocol.into()),
                                    format: serde_json::json!(format),
                                    new_track: None,
                                    additional_data: serde_json::Value::Object(serde_json::Map::new()),
                                    exception: None,
                                });
                            }
                        }
                        break;
                    }
                }
            }
        }

        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: serde_json::Value::Null,
            new_track: None,
            additional_data: serde_json::Value::Object(serde_json::Map::new()),
            exception: Some("VK stream not found.".into()),
        })
    }
}

impl VKMusicSource {
    async fn recommendations_inner(&self, query: &str) -> anyhow::Result<SourceResult> {
        let audio_id = if Regex::new(r"^-?\d+_\d+$").unwrap().is_match(query) {
            query.to_string()
        } else {
            match self.search(query, Some("vksearch")).await? {
                SourceResult::Search { ref data } if !data.is_empty() => {
                    data[0].info.identifier.clone()
                }
                _ => return Ok(SourceResult::Empty),
            }
        };

        match self
            .api_request::<Value>(
                "audio.getRecommendations",
                vec![("target_audio", &audio_id), ("count", "20"), ("extended", "1")],
            )
            .await
        {
            Some(data) => {
                let items = data.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
                let tracks: Vec<TrackData> = items.iter().filter_map(Self::build_track).collect();
                if tracks.is_empty() {
                    Ok(SourceResult::Empty)
                } else {
                    Ok(SourceResult::Playlist {
                        data: PlaylistData {
                            encoded: String::new(),
                            info: PlaylistInfo {
                                name: "VK Recommendations".into(),
                                selected_track: 0,
                            },
                            plugin_info: serde_json::json!({"type": "recommendations"}),
                            tracks,
                        },
                    })
                }
            }
            None => Ok(SourceResult::Empty),
        }
    }

    async fn resolve_playlist_inner(
        &self,
        owner_id: &str,
        playlist_id: Option<&str>,
        access_key: Option<&str>,
    ) -> anyhow::Result<SourceResult> {
        // Try API
        if self.get_auth().is_some() {
            let mut params = vec![("owner_id", owner_id), ("extended", "1"), ("count", "100")];
            if let Some(pid) = playlist_id {
                params.push(("album_id", pid));
            }
            if let Some(ak) = access_key {
                params.push(("access_key", ak));
            }

            if let Some(data) = self.api_request::<Value>("audio.get", params).await {
                let items = data.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
                if !items.is_empty() {
                    let tracks: Vec<TrackData> = items.iter().filter_map(Self::build_track).collect();
                    return Ok(SourceResult::Playlist {
                        data: PlaylistData {
                            encoded: String::new(),
                            info: PlaylistInfo {
                                name: "VK Playlist".into(),
                                selected_track: 0,
                            },
                            plugin_info: serde_json::json!({"type": "playlist"}),
                            tracks,
                        },
                    });
                }
            }
        }

        // Scrape fallback
        self.scrape_playlist(owner_id).await
    }

    async fn resolve_track_inner(
        &self,
        url: &str,
        caps: &regex::Captures<'_>,
    ) -> anyhow::Result<SourceResult> {
        // Try API
        if self.get_auth().is_some() {
            let owner = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let hash = caps.get(3).map(|m| m.as_str());
            let audios = match hash {
                Some(h) => format!("{}_{}_{}", owner, id, h),
                None => format!("{}_{}", owner, id),
            };

            if let Some(items) = self
                .api_request::<Vec<Value>>("audio.getById", vec![("audios", &audios), ("extended", "1")])
                .await
            {
                if let Some(item) = items.first() {
                    let mut track = Self::build_track(item);
                    // Self-heal missing artwork
                    if let Some(ref t) = track {
                        if t.info.artwork_url.is_none() {
                            if let Some(search_data) = self
                                .api_request::<Value>(
                                    "audio.search",
                                    vec![
                                        ("q", &format!("{} {}", t.info.author, t.info.title)),
                                        ("count", "10"),
                                    ],
                                )
                                .await
                            {
                                if let Some(search_items) = search_data.get("items").and_then(|i| i.as_array()) {
                                    if let Some(heal_item) = search_items.first().and_then(|i| {
                                        let i_url = Self::build_track(i);
                                        i_url.filter(|t2| t2.info.artwork_url.is_some())
                                    }) {
                                        track = Some(heal_item);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(t) = track {
                        return Ok(SourceResult::Track(t));
                    }
                }
            }
        }

        // Scrape fallback
        match self.scrape_track(url).await? {
            Some(track) => Ok(SourceResult::Track(track)),
            None => Ok(SourceResult::Empty),
        }
    }
}

fn cookie_auth_possible(body: &Value) -> bool {
    body.get("error")
        .and_then(|e| e.get("error_code"))
        .and_then(|c| c.as_i64())
        .map(|c| c == 5)
        .unwrap_or(false)
}

impl Clone for VkAuth {
    fn clone(&self) -> Self {
        Self {
            access_token: self.access_token.clone(),
            user_id: self.user_id,
            cookie: self.cookie.clone(),
        }
    }
}
