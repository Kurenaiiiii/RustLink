use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::sync::Mutex;
use std::time::Instant;

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const BOT_UA: &str = "Mozilla/5.0 (compatible; NodeLinkBot/0.1; +https://nodelink.js.org/)";
const SEARCH_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36";

const CONFIG_TTL_MS: u128 = 60_000;

#[derive(Clone)]
struct AmzConfig {
    access_token: String,
    csrf_token: String,
    csrf_ts: String,
    csrf_rnd: String,
    device_id: String,
    session_id: String,
}

struct ConfigCache {
    data: AmzConfig,
    fetched_at: Instant,
}

pub struct AmazonMusicSource {
    client: Client,
    config: Mutex<Option<ConfigCache>>,
}

impl AmazonMusicSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            config: Mutex::new(None),
        }
    }

    fn patterns() -> Vec<Regex> {
        vec![
            Regex::new(r"^https?://music\.amazon\.[a-z.]+/(?:.*/)?(track|album|playlist|artist)s?/([a-z0-9]+)").unwrap(),
            Regex::new(r"^https?://(?:www\.)?amazon\.[a-z.]+/dp/([a-z0-9]+)").unwrap(),
        ]
    }

    async fn get_or_fetch_config(&self) -> Option<AmzConfig> {
        {
            let cache = self.config.lock().unwrap();
            if let Some(c) = cache.as_ref() {
                if c.fetched_at.elapsed().as_millis() < CONFIG_TTL_MS {
                    return Some(c.data.clone());
                }
            }
        }

        let cfg = Self::fetch_config(&self.client).await;
        if let Some(ref data) = cfg {
            self.config.lock().unwrap().replace(ConfigCache {
                data: data.clone(),
                fetched_at: Instant::now(),
            });
        }
        cfg
    }

    async fn fetch_config(client: &Client) -> Option<AmzConfig> {
        let resp = client
            .get("https://music.amazon.com/config.json")
            .header("User-Agent", SEARCH_UA)
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        let text = resp.text().await.ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;

        let csrf = v.get("csrf")?;
        let csrf_token = csrf.get("token")?.as_str()?.to_string();
        let csrf_ts = csrf.get("ts")?.as_str()?.to_string();
        let csrf_rnd = csrf.get("rnd")?.as_str()?.to_string();

        let access_token = v.get("accessToken").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let device_id = v.get("deviceId").and_then(|d| d.as_str()).map(|s| s.to_string()).unwrap_or_else(|| "13580682033287541".into());
        let session_id = v.get("sessionId").and_then(|s| s.as_str()).map(|s| s.to_string()).unwrap_or_else(|| "142-4001091-4160417".into());

        Some(AmzConfig {
            access_token,
            csrf_token,
            csrf_ts,
            csrf_rnd,
            device_id,
            session_id,
        })
    }

    fn build_amzn_headers(cfg: &AmzConfig, page_url: &str) -> Vec<(String, String)> {
        let csrf_header = serde_json::json!({
            "interface": "CSRFInterface.v1_0.CSRFHeaderElement",
            "token": cfg.csrf_token,
            "timestamp": cfg.csrf_ts,
            "rndNonce": cfg.csrf_rnd,
        });

        let auth_header = serde_json::json!({
            "interface": "ClientAuthenticationInterface.v1_0.ClientTokenElement",
            "accessToken": cfg.access_token,
        });

        vec![
            ("x-amzn-authentication".into(), auth_header.to_string()),
            ("x-amzn-device-model".into(), "WEBPLAYER".into()),
            ("x-amzn-device-width".into(), "1920".into()),
            ("x-amzn-device-height".into(), "1080".into()),
            ("x-amzn-device-family".into(), "WebPlayer".into()),
            ("x-amzn-device-id".into(), cfg.device_id.clone()),
            ("x-amzn-user-agent".into(), SEARCH_UA.into()),
            ("x-amzn-session-id".into(), cfg.session_id.clone()),
            ("x-amzn-request-id".into(), uuid::Uuid::new_v4().to_string()),
            ("x-amzn-device-language".into(), "en_US".into()),
            ("x-amzn-currency-of-preference".into(), "USD".into()),
            ("x-amzn-os-version".into(), "1.0".into()),
            ("x-amzn-application-version".into(), "1.0.9172.0".into()),
            ("x-amzn-device-time-zone".into(), "America/New_York".into()),
            ("x-amzn-timestamp".into(), format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())),
            ("x-amzn-csrf".into(), csrf_header.to_string()),
            ("x-amzn-music-domain".into(), "music.amazon.com".into()),
            ("x-amzn-page-url".into(), page_url.into()),
            ("x-amzn-feature-flags".into(), "hd-supported,uhd-supported".into()),
        ]
    }

    async fn fetch_jsonld(&self, url: &str) -> Option<SourceResult> {
        let resp = self.client.get(url).header("User-Agent", BOT_UA).send().await.ok()?;
        if resp.status() != 200 {
            return None;
        }
        let html = resp.text().await.ok()?;

        let header_artist = Regex::new(r#"<music-detail-header[^>]*primary-text="([^"]+)""#)
            .ok().and_then(|re| re.captures(&html))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().replace("&amp;", "&"));

        let header_image = Regex::new(r#"<music-detail-header[^>]*image-src="([^"]+)""#)
            .ok().and_then(|re| re.captures(&html))
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));

        let og_image = Regex::new(r#"<meta property="og:image" content="([^"]+)""#)
            .ok().and_then(|re| re.captures(&html))
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));

        let artwork_url = header_image.or(og_image);

        let jsonld_re = Regex::new(r#"<script [^>]*type="application/ld\+json"[^>]*>([\s\S]*?)</script>"#).ok()?;

        let mut collection_type: Option<String> = None;
        let mut collection_name: String = header_artist.clone().unwrap_or_else(|| "Unknown Artist".into());
        let mut collection_image: Option<String> = artwork_url.clone();
        let mut tracks: Vec<TrackInfo> = vec![];

        for cap in jsonld_re.captures_iter(&html) {
            let content = cap[1].replace("&quot;", "\"").replace("&amp;", "&");
            let parsed: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let entries = match parsed {
                Value::Array(ref arr) => arr.clone(),
                _ => vec![parsed.clone()],
            };

            for entry in &entries {
                let ty = entry.get("@type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "MusicAlbum" | "MusicGroup" | "Playlist" => {
                        if let Some(n) = entry.get("name").and_then(|n| n.as_str()) {
                            collection_name = n.to_string();
                        }
                        if let Some(img) = entry.get("image").and_then(|i| i.as_str()) {
                            collection_image = Some(img.to_string());
                        }
                        collection_type = Some(ty.to_string());

                        if let Some(artist) = entry.get("byArtist") {
                            if let Some(name) = artist.get("name").and_then(|n| n.as_str()) {
                                collection_name = name.to_string();
                            } else if let Some(arr) = artist.as_array() {
                                if let Some(first) = arr.first().and_then(|a| a.get("name").and_then(|n| n.as_str())) {
                                    collection_name = first.to_string();
                                }
                            }
                        }
                        if let Some(author) = entry.get("author").and_then(|a| a.get("name").and_then(|n| n.as_str())) {
                            collection_name = author.to_string();
                        }

                        if let Some(track_list) = entry.get("track").and_then(|t| t.as_array()) {
                            for t_entry in track_list {
                                let t_name = t_entry.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown").to_string();
                                let t_url = t_entry.get("url").and_then(|u| u.as_str()).unwrap_or(url);
                                let t_id = t_url.split('/').last().unwrap_or(&t_name);
                                let t_artist = t_entry.get("byArtist").and_then(|a| {
                                    if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                                        Some(name.to_string())
                                    } else if let Some(arr) = a.as_array() {
                                        arr.first().and_then(|f| f.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                                    } else { None }
                                }).unwrap_or_else(|| collection_name.clone());

                                let t_duration = t_entry.get("duration").and_then(|d| d.as_str()).map(|s| Self::parse_iso8601(s)).unwrap_or(0);
                                let t_isrc = t_entry.get("isrcCode").and_then(|i| i.as_str()).map(|s| s.to_string());

                                tracks.push(TrackInfo {
                                    identifier: t_id.to_string(),
                                    is_seekable: true,
                                    author: t_artist,
                                    length: t_duration,
                                    is_stream: false,
                                    position: 0,
                                    title: t_name,
                                    uri: Some(t_url.to_string()),
                                    artwork_url: collection_image.clone(),
                                    isrc: t_isrc,
                                    source_name: "amazonmusic".into(),
                                    chapters: None,
                                });
                            }
                        }
                    }
                    "MusicRecording" => {
                        let t_name = entry.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown").to_string();
                        let t_artist = entry.get("byArtist").and_then(|a| {
                            if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                                Some(name.to_string())
                            } else if let Some(arr) = a.as_array() {
                                arr.first().and_then(|f| f.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                            } else { None }
                        }).or_else(|| entry.get("author").and_then(|a| a.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())))
                        .unwrap_or_else(|| "Unknown Artist".into());

                        let t_duration = entry.get("duration").and_then(|d| d.as_str()).map(|s| Self::parse_iso8601(s)).unwrap_or(0);
                        let t_isrc = entry.get("isrcCode").and_then(|i| i.as_str()).map(|s| s.to_string());
                        let t_id = entry.get("id").and_then(|i| i.as_str()).or_else(|| entry.get("@id").and_then(|i| i.as_str())).unwrap_or("am-unknown").split('/').last().unwrap_or("am-unknown");

                        let track = TrackInfo {
                            identifier: t_id.to_string(),
                            is_seekable: true,
                            author: t_artist,
                            length: t_duration,
                            is_stream: false,
                            position: 0,
                            title: t_name,
                            uri: Some(url.to_string()),
                            artwork_url: artwork_url.clone(),
                            isrc: t_isrc,
                            source_name: "amazonmusic".into(),
                            chapters: None,
                        };

                        let mut td = TrackData {
                            encoded: None,
                            info: track,
                            plugin_info: serde_json::json!({}),
                            user_data: serde_json::json!({}),
                            details: Vec::new(),
                            message_flags: 0,
                        };
                        td.encoded = Some(encode_track(&td));
                        return Some(SourceResult::Track(td));
                    }
                    _ => {}
                }
            }
        }

        if tracks.is_empty() {
            let row_re = Regex::new(r#"<(music-image-row|music-text-row)[^>]*primary-text="([^"]+)"[^>]*primary-href="([^"]+)"(?:[^>]*secondary-text-1="([^"]+)")?[^>]*duration="([^"]+)"(?:[^>]*image-src="([^"]+)")?"#).ok()?;
            for cap in row_re.captures_iter(&html) {
                let t_title = cap[2].replace("&amp;", "&");
                if t_title.is_empty() { continue; }
                let t_href = &cap[3];
                let t_artist = cap.get(4).map(|m| m.as_str().replace("&amp;", "&")).unwrap_or_else(|| collection_name.clone());
                let t_duration = &cap[5];
                let t_image = cap.get(6).map(|m| m.as_str().to_string()).or_else(|| collection_image.clone());
                let t_id = Self::extract_identifier(t_href).unwrap_or_else(|| format!("am-{}", hex::encode(t_title.as_bytes())));

                let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
                let length = if t_duration.contains(':') {
                    Self::parse_colon_duration(t_duration)
                } else { 0 };

                tracks.push(TrackInfo {
                    identifier: t_id.clone(),
                    is_seekable: true,
                    author: t_artist,
                    length,
                    is_stream: false,
                    position: 0,
                    title: t_title,
                    uri: Some(format!("{}/tracks/{}", origin, t_id)),
                    artwork_url: t_image,
                    isrc: None,
                    source_name: "amazonmusic".into(),
                    chapters: None,
                });
            }
        }

        if tracks.is_empty() {
            return None;
        }

        if tracks.len() == 1 {
            let track = tracks.remove(0);
            let mut td = TrackData {
                encoded: None,
                info: track,
                plugin_info: serde_json::json!({}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            td.encoded = Some(encode_track(&td));
            return Some(SourceResult::Track(td));
        }

        let ct = collection_type.as_deref().unwrap_or("playlist");
        let playlist_type = match ct {
            "MusicAlbum" => "album",
            "MusicGroup" => "artist",
            _ => "playlist",
        };

        let playlist_tracks: Vec<TrackData> = tracks.into_iter().map(|info| {
            let mut td = TrackData {
                encoded: None,
                info,
                plugin_info: serde_json::json!({}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            td.encoded = Some(encode_track(&td));
            td
        }).collect();

        let encoded = playlist_tracks.first().and_then(|t| t.encoded.clone()).unwrap_or_default();
        Some(SourceResult::Playlist {
            data: PlaylistData {
                encoded,
                info: PlaylistInfo {
                    name: collection_name,
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": playlist_type}),
                tracks: playlist_tracks,
            },
        })
    }

    fn extract_identifier(deeplink: &str) -> Option<String> {
        let stripped = deeplink.split('?').next().unwrap_or(deeplink);
        let stripped = stripped.split('#').next().unwrap_or(stripped);
        stripped.rsplit('/').next().map(|s| s.to_string())
    }

    fn parse_iso8601(duration: &str) -> i64 {
        let re = match Regex::new(r"PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?") {
            Ok(r) => r,
            Err(_) => return 0,
        };
        let caps = match re.captures(duration) {
            Some(c) => c,
            None => return 0,
        };
        let h: i64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let m: i64 = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
        let s: i64 = caps.get(3).and_then(|s| s.as_str().parse().ok()).unwrap_or(0);
        (h * 3600 + m * 60 + s) * 1000
    }

    fn parse_colon_duration(s: &str) -> i64 {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let m: i64 = parts[0].parse().unwrap_or(0);
            let s: i64 = parts[1].parse().unwrap_or(0);
            (m * 60 + s) * 1000
        } else if parts.len() == 3 {
            let h: i64 = parts[0].parse().unwrap_or(0);
            let m: i64 = parts[1].parse().unwrap_or(0);
            let s: i64 = parts[2].parse().unwrap_or(0);
            (h * 3600 + m * 60 + s) * 1000
        } else {
            0
        }
    }
}

#[async_trait]
impl SourceProvider for AmazonMusicSource {
    fn name(&self) -> &'static str {
        "amazonmusic"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["azmusic", "amz"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["amazonmusic", "azsearch"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let cfg = match self.get_or_fetch_config().await {
            Some(c) => c,
            None => return Ok(SourceResult::Empty),
        };

        let page_url = format!("https://music.amazon.com/search/{}?filter=IsLibrary%7Cfalse&sc=none", urlencoding(query));
        let amzn_headers = Self::build_amzn_headers(&cfg, &page_url);

        let search_payload = serde_json::json!({
            "filter": "{\"IsLibrary\":[\"false\"]}",
            "keyword": "{\"interface\":\"Web.TemplatesInterface.v1_0.Touch.SearchTemplateInterface.SearchKeywordClientInformation\",\"keyword\":\"\"}",
            "suggestedKeyword": query,
            "userHash": "{\"level\":\"LIBRARY_MEMBER\"}",
            "headers": serde_json::Value::Object(amzn_headers.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect()),
        });

        let req = self.client
            .post("https://na.mesk.skill.music.a2z.com/api/showSearch")
            .header("Content-Type", "text/plain;charset=UTF-8")
            .header("x-amzn-csrf", &cfg.csrf_token)
            .header("Origin", "https://music.amazon.com")
            .header("User-Agent", SEARCH_UA)
            .body(search_payload.to_string());

        let resp = req.send().await?;
        if resp.status() != 200 {
            return Ok(SourceResult::Empty);
        }

        let text = resp.text().await?;
        let data: Value = serde_json::from_str(&text).unwrap_or_default();
        let widgets = data.get("methods")
            .and_then(|m| m.as_array())
            .and_then(|arr| arr.first())
            .and_then(|m| m.get("template"))
            .and_then(|t| t.get("widgets"))
            .and_then(|w| w.as_array());

        let widgets = match widgets {
            Some(w) => w,
            None => return Ok(SourceResult::Empty),
        };

        let mut tracks: Vec<TrackInfo> = vec![];
        for widget in widgets {
            let items = match widget.get("items").and_then(|i| i.as_array()) {
                Some(i) => i,
                None => continue,
            };
            for item in items {
                let label = item.get("label").and_then(|l| l.as_str()).unwrap_or("");
                if label != "song" { continue; }

                let primary_link = item.get("primaryLink").and_then(|p| p.get("deeplink")).and_then(|d| d.as_str());
                let identifier = primary_link.and_then(Self::extract_identifier);
                let ident = match identifier {
                    Some(id) => id,
                    None => continue,
                };

                let author = item.get("secondaryText")
                    .and_then(|s| {
                        if let Some(obj) = s.as_object() {
                            obj.get("text").and_then(|t| t.as_str())
                        } else {
                            s.as_str()
                        }
                    })
                    .unwrap_or("Unknown Artist")
                    .to_string();

                let title = item.get("primaryText")
                    .and_then(|p| {
                        if let Some(obj) = p.as_object() {
                            obj.get("text").and_then(|t| t.as_str())
                        } else {
                            p.as_str()
                        }
                    })
                    .unwrap_or("Unknown Track")
                    .to_string();

                tracks.push(TrackInfo {
                    identifier: ident.clone(),
                    is_seekable: true,
                    author,
                    length: 0,
                    is_stream: false,
                    position: 0,
                    title,
                    uri: Some(format!("https://music.amazon.com/tracks/{}", ident)),
                    artwork_url: item.get("image").and_then(|i| i.as_str()).map(|s| s.to_string()),
                    isrc: None,
                    source_name: "amazonmusic".into(),
                    chapters: None,
                });
            }
        }

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let result_tracks: Vec<TrackData> = tracks.into_iter().map(|info| {
            let mut td = TrackData {
                encoded: None,
                info,
                plugin_info: serde_json::json!({}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            td.encoded = Some(encode_track(&td));
            td
        }).collect();

        Ok(SourceResult::Search { data: result_tracks })
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let patterns = Self::patterns();
        let mut matched = None;
        for p in &patterns {
            if let Some(caps) = p.captures(query) {
                if caps.len() >= 3 {
                    matched = Some((caps[1].to_string(), caps[2].to_string()));
                } else {
                    matched = Some(("track".into(), caps[1].to_string()));
                }
                break;
            }
        }

        let (_typ, id) = match matched {
            Some(m) => m,
            None => return Ok(SourceResult::Empty),
        };

        if let Some(result) = self.fetch_jsonld(query).await {
            return Ok(result);
        }

        // Fallback: Odesli
        let api_url = format!("https://api.song.link/v1-alpha.1/links?url={}", urlencoding(query.split('?').next().unwrap_or(query)));
        if let Ok(resp) = self.client.get(&api_url).header("User-Agent", BOT_UA).send().await {
            if resp.status() == 200 {
                if let Ok(text) = resp.text().await {
                    if let Ok(data) = serde_json::from_str::<Value>(&text) {
                        if let Some(entities) = data.get("entitiesByUniqueId").and_then(|e| e.as_object()) {
                            let unique_id = data.get("entityUniqueId").and_then(|u| u.as_str());
                            if let Some(entity) = unique_id.and_then(|uid| entities.get(uid)).or_else(|| {
                                entities.values().find(|e| {
                                    e.get("id").and_then(|i| i.as_str()).map(|i| i.contains(&id)).unwrap_or(false)
                                })
                            }) {
                                let title = entity.get("title").and_then(|t| t.as_str()).unwrap_or("Unknown").to_string();
                                let artist = entity.get("artistName").and_then(|a| a.as_str()).unwrap_or("Unknown Artist").to_string();
                                let thumbnail = entity.get("thumbnailUrl").and_then(|t| t.as_str()).map(|s| s.to_string());
                                let isrc = entity.get("isrc").and_then(|i| i.as_str()).map(|s| s.to_string());
                                let entity_id = entity.get("id").and_then(|i| i.as_str()).unwrap_or(&id).to_string();

                                let info = TrackInfo {
                                    identifier: entity_id,
                                    is_seekable: true,
                                    author: artist,
                                    length: 0,
                                    is_stream: false,
                                    position: 0,
                                    title,
                                    uri: Some(query.to_string()),
                                    artwork_url: thumbnail,
                                    isrc,
                                    source_name: "amazonmusic".into(),
                                    chapters: None,
                                };
                                let mut td = TrackData {
                                    encoded: None,
                                    info,
                                    plugin_info: serde_json::json!({}),
                                    user_data: serde_json::json!({}),
                                    details: Vec::new(),
                                    message_flags: 0,
                                };
                                td.encoded = Some(encode_track(&td));
                                return Ok(SourceResult::Track(td));
                            }
                        }
                    }
                }
            }
        }

        Ok(SourceResult::Empty)
    }

    async fn get_track_url(&self, _track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        // Amazon Music doesn't provide direct streaming URLs
        // Return metadata only - client handles mirror resolution
        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: serde_json::json!({}),
            new_track: None,
            additional_data: serde_json::json!({}),
            exception: Some("Amazon Music does not provide direct stream URLs. Use ISRC for mirror resolution.".into()),
        })
    }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
