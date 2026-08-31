use async_trait::async_trait;
use md5::{Digest, Md5};
use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const API_BASE: &str = "https://api.music.yandex.net";
const USER_AGENT: &str = "Yandex-Music-API";
const CLIENT_HEADER: &str = "YandexMusicAndroid/24023621";
const MD5_SECRET: &str = "XGRlBW9FXlekgbPrRHuSiA";
const ARTIST_MAX: usize = 10;
const PLAYLIST_MAX: usize = 100;
const ALBUM_MAX: usize = 50;

pub struct YandexMusicSource {
    client: Client,
    token: std::sync::Mutex<Option<String>>,
    artist_ll: usize,
    album_ll: usize,
    playlist_ll: usize,
    allow_unavail: bool,
}

impl YandexMusicSource {
    pub fn new(
        token: Option<String>,
        artist_ll: usize,
        album_ll: usize,
        playlist_ll: usize,
        allow_unavail: bool,
    ) -> Self {
        Self {
            client: Client::new(),
            token: std::sync::Mutex::new(token),
            artist_ll,
            album_ll,
            playlist_ll,
            allow_unavail,
        }
    }

    fn get_token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    fn patterns() -> &'static [Regex; 3] {
        static P: std::sync::OnceLock<[Regex; 3]> = std::sync::OnceLock::new();
        P.get_or_init(|| {
            [
                Regex::new(r"music\.yandex\.(ru|com|kz|by)/(artist|album|track)/(\d+)(?:/track/(\d+))?").unwrap(),
                Regex::new(r"music\.yandex\.(ru|com|kz|by)/users/([0-9A-Za-z@.-]+)/playlists/(\d+)").unwrap(),
                Regex::new(r"music\.yandex\.(ru|com|kz|by)/playlists/([0-9A-Za-z.-]+)").unwrap(),
            ]
        })
    }

    async fn api<T: serde::de::DeserializeOwned>(&self, path: &str, params: Vec<(&str, &str)>) -> anyhow::Result<T> {
        let t = self.get_token().ok_or_else(|| anyhow::anyhow!("Yandex Music token required"))?;
        let url = reqwest::Url::parse_with_params(&format!("{}{}", API_BASE, path), &params)?;
        let r = self.client
            .get(url)
            .header("Accept", "application/json")
            .header("Authorization", format!("OAuth {}", t))
            .header("User-Agent", USER_AGENT)
            .header("X-Yandex-Music-Client", CLIENT_HEADER)
            .send()
            .await?;
        if !r.status().is_success() {
            anyhow::bail!("Yandex API HTTP {} for {}", r.status(), path);
        }
        let body: Value = r.json().await?;
        let result = body.get("result").ok_or_else(|| anyhow::anyhow!("No result field"))?;
        Ok(serde_json::from_value(result.clone())?)
    }

    async fn api_raw(&self, path: &str, params: Vec<(&str, &str)>) -> anyhow::Result<Value> {
        let t = self.get_token().ok_or_else(|| anyhow::anyhow!("Yandex Music token required"))?;
        let url = reqwest::Url::parse_with_params(&format!("{}{}", API_BASE, path), &params)?;
        let r = self.client
            .get(url)
            .header("Accept", "application/json")
            .header("Authorization", format!("OAuth {}", t))
            .header("User-Agent", USER_AGENT)
            .header("X-Yandex-Music-Client", CLIENT_HEADER)
            .send()
            .await?;
        if !r.status().is_success() {
            anyhow::bail!("Yandex API HTTP {} for {}", r.status(), path);
        }
        Ok(r.json().await?)
    }

    async fn fetch_text(&self, url: &str) -> anyhow::Result<String> {
        let t = self.get_token().ok_or_else(|| anyhow::anyhow!("Yandex Music token required"))?;
        let r = self.client
            .get(url)
            .header("Authorization", format!("OAuth {}", t))
            .send()
            .await?;
        if !r.status().is_success() {
            anyhow::bail!("Fetch text HTTP {}", r.status());
        }
        Ok(r.text().await?)
    }

    async fn get_download_url(&self, track_id: &str) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct Di {
            codec: String,
            #[serde(rename = "bitrateInKbps")]
            bitrate: Option<i64>,
            #[serde(rename = "downloadInfoUrl")]
            info_url: String,
        }
        let infos: Vec<Di> = self.api(&format!("/tracks/{}/download-info", track_id), vec![]).await?;
        let best = infos.iter()
            .filter(|i| i.codec == "mp3")
            .max_by_key(|i| i.bitrate.unwrap_or(0))
            .ok_or_else(|| anyhow::anyhow!("No MP3 for track {}", track_id))?;

        let xml = self.fetch_text(&best.info_url).await?;
        let host = read_tag(&xml, "host")?;
        let path = read_tag(&xml, "path")?;
        let ts = read_tag(&xml, "ts")?;
        let s = read_tag(&xml, "s")?;

        let sign = format!("{}{}{}", MD5_SECRET, path, s);
        let md5 = hex::encode(Md5::digest(sign.as_bytes()));
        Ok(format!("https://{}/get-mp3/{}/{}{}", host, md5, ts, path))
    }

    fn parse_tracks(list: &Value, domain: &str, allow_unavail: bool) -> Vec<TrackData> {
        list.as_array().map(|arr| {
            arr.iter().filter_map(|item| {
                let node = item.get("track").unwrap_or(item);
                Self::parse_track(node, domain, allow_unavail)
            }).collect()
        }).unwrap_or_default()
    }

    fn parse_track(json: &Value, domain: &str, allow_unavail: bool) -> Option<TrackData> {
        let available = json.get("available").and_then(|v| v.as_bool()).unwrap_or(true);
        if !available && !allow_unavail {
            return None;
        }

        let id = json.get("id").and_then(|v| {
            v.as_i64().map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        }).unwrap_or_else(|| "0".into());

        let title = json.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown Title");

        let author = {
            let is_podcast = json.get("major")
                .and_then(|m| m.get("name").and_then(|n| n.as_str())) == Some("PODCASTS");
            if is_podcast {
                json.get("albums").and_then(|a| a.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|a| a.get("title").and_then(|t| t.as_str()))
                    .unwrap_or("Unknown Artist")
                    .to_string()
            } else {
                json.get("artists").and_then(|a| a.as_array())
                    .map(|arr| arr.iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_else(|| "Unknown Artist".into())
            }
        };

        let duration = json.get("durationMs").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;

        let album_title = json.get("albums").and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.get("title").and_then(|t| t.as_str()));
        let album_id = json.get("albums").and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.get("id").and_then(|id| id.as_i64()));

        let artwork_url = parse_cover(json);

        let isrc = json.get("isrc").and_then(|v| v.as_str()).map(|s| s.to_string());

        let uri = format!("https://music.yandex.{}/track/{}", domain, id);

        let info = TrackInfo {
            identifier: id,
            is_seekable: true,
            author,
            length: duration,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri: Some(uri),
            artwork_url,
            isrc,
            source_name: "yandexmusic".into(),
            chapters: None,
        };

        let mut plugin = serde_json::Map::new();
        if let Some(at) = album_title {
            plugin.insert("albumName".into(), Value::String(at.to_string()));
        }
        if let Some(aid) = album_id {
            plugin.insert("albumUrl".into(), Value::String(format!("https://music.yandex.{}/album/{}", domain, aid)));
        }
        if json.get("available").and_then(|v| v.as_bool()) == Some(false) {
            plugin.insert("unavailable".into(), Value::Bool(true));
        }

        let encoded = encode_track(&TrackData {
            encoded: None,
            info: info.clone(),
            plugin_info: Value::Object(plugin.clone()),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        });

        Some(TrackData {
            encoded: Some(encoded),
            info,
            plugin_info: Value::Object(plugin),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        })
    }

    async fn recommendations_inner(&self, query: &str) -> anyhow::Result<SourceResult> {
        let track_id = if Regex::new(r"^\d+$").unwrap().is_match(query) {
            query.to_string()
        } else {
            match self._search_tracks(query, 1).await? {
                SourceResult::Search { data } if !data.is_empty() => data[0].info.identifier.clone(),
                _ => return Ok(SourceResult::Empty),
            }
        };

        let data = self.api_raw(&format!("/tracks/{}/similar", track_id), vec![]).await?;
        let similar = data.get("similarTracks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if similar.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let tracks = Self::parse_tracks(&Value::Array(similar), "com", self.allow_unavail);
        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo { name: "Yandex Music Recommendations".into(), selected_track: 0 },
                plugin_info: serde_json::json!({"type": "recommendations"}),
                tracks,
            },
        })
    }

    async fn _search_tracks(&self, query: &str, limit: usize) -> anyhow::Result<SourceResult> {
        let data = self.api_raw("/search", vec![("text", query), ("type", "all"), ("page", "0")]).await?;
        let tracks = data.get("tracks").and_then(|t| t.get("results")).cloned().unwrap_or(Value::Null);
        let tracks = Self::parse_tracks(&tracks, "com", self.allow_unavail);
        let tracks: Vec<TrackData> = tracks.into_iter().take(limit).collect();
        if tracks.is_empty() { Ok(SourceResult::Empty) } else { Ok(SourceResult::Search { data: tracks }) }
    }

    async fn resolve_track(&self, id: &str, domain: &str) -> anyhow::Result<SourceResult> {
        match self.api::<Vec<Value>>(&format!("/tracks/{}", id), vec![]).await {
            Ok(nodes) => {
                let node = nodes.first().ok_or_else(|| anyhow::anyhow!("Empty track result"))?;
                match Self::parse_track(node, domain, self.allow_unavail) {
                    Some(t) => Ok(SourceResult::Track(t)),
                    None => Ok(SourceResult::Empty),
                }
            }
            Err(e) => Ok(SourceResult::Error(format!("Track resolve failed: {}", e))),
        }
    }

    async fn resolve_album(&self, id: &str, domain: &str) -> anyhow::Result<SourceResult> {
        let page_size = (ALBUM_MAX * self.album_ll.max(1)).to_string();
        let data = self.api_raw(&format!("/albums/{}/with-tracks", id), vec![("page-size", &page_size)]).await?;

        let title = data.get("title").and_then(|v| v.as_str()).unwrap_or("Yandex Music Album");
        let volumes = data.get("volumes").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        let mut tracks = Vec::new();
        for vol in &volumes {
            let parsed = Self::parse_tracks(vol, domain, self.allow_unavail);
            tracks.extend(parsed);
        }

        if tracks.is_empty() { return Ok(SourceResult::Empty); }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo { name: title.into(), selected_track: 0 },
                plugin_info: serde_json::json!({"type": "album"}),
                tracks,
            },
        })
    }

    async fn resolve_artist(&self, id: &str, domain: &str) -> anyhow::Result<SourceResult> {
        let page_size = (ARTIST_MAX * self.artist_ll.max(1)).to_string();
        let data = self.api_raw(&format!("/artists/{}/tracks", id), vec![("page-size", &page_size)]).await?;
        let tracks_arr = data.get("tracks").cloned().unwrap_or(Value::Null);
        let tracks = Self::parse_tracks(&tracks_arr, domain, self.allow_unavail);

        if tracks.is_empty() { return Ok(SourceResult::Empty); }

        let name = self.api_raw(&format!("/artists/{}", id), vec![]).await
            .ok()
            .and_then(|v| {
                let name_str = v
                    .get("artist")
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str());
                name_str.map(|s| s.to_string())
            })
            .unwrap_or_else(|| "Unknown Artist".to_string());

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo { name: format!("{}'s Top Tracks", name), selected_track: 0 },
                plugin_info: serde_json::json!({"type": "artist"}),
                tracks,
            },
        })
    }

    async fn resolve_playlist(&self, user: &str, id: &str, domain: &str) -> anyhow::Result<SourceResult> {
        let page_size = (PLAYLIST_MAX * self.playlist_ll.max(1)).to_string();
        let data = self.api_raw(
            &format!("/users/{}/playlists/{}", user, id),
            vec![("page-size", &page_size), ("rich-tracks", "true")],
        ).await?;

        Self::parse_playlist_result(&data, domain, self.allow_unavail)
    }

    async fn resolve_playlist_uuid(&self, uuid: &str, domain: &str) -> anyhow::Result<SourceResult> {
        let page_size = (PLAYLIST_MAX * self.playlist_ll.max(1)).to_string();
        let data = self.api_raw(
            &format!("/playlist/{}", uuid),
            vec![("page-size", &page_size), ("rich-tracks", "true")],
        ).await?;

        Self::parse_playlist_result(&data, domain, self.allow_unavail)
    }

    fn parse_playlist_result(data: &Value, domain: &str, allow_unavail: bool) -> anyhow::Result<SourceResult> {
        let tracks_arr = data.get("tracks").cloned().unwrap_or(Value::Null);
        let tracks = Self::parse_tracks(&tracks_arr, domain, allow_unavail);
        if tracks.is_empty() { return Ok(SourceResult::Empty); }

        let owner_name = data.get("owner")
            .and_then(|o| o.get("name").or_else(|| o.get("login")).and_then(|v| v.as_str()))
            .unwrap_or("Unknown");

        let kind = data.get("kind").and_then(|v| v.as_i64()).unwrap_or(0);
        let title = if kind == 3 {
            format!("{}'s Liked Songs", owner_name)
        } else {
            data.get("title").and_then(|v| v.as_str()).unwrap_or("Yandex Music Playlist").to_string()
        };

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo { name: title, selected_track: 0 },
                plugin_info: serde_json::json!({"type": "playlist"}),
                tracks,
            },
        })
    }

    fn build_search_result(title: &str, author: &str, id: &str, uri: &str, artwork: Option<String>, ptype: &str) -> TrackData {
        let info = TrackInfo {
            identifier: id.to_string(),
            is_seekable: ptype != "artist",
            author: author.to_string(),
            length: 0,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri: Some(uri.to_string()),
            artwork_url: artwork,
            isrc: None,
            source_name: "yandexmusic".into(),
            chapters: None,
        };
        let encoded = encode_track(&TrackData {
            encoded: None,
            info: info.clone(),
            plugin_info: serde_json::json!({"type": ptype}),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        });
        TrackData {
            encoded: Some(encoded),
            info,
            plugin_info: serde_json::json!({"type": ptype}),
            user_data: Value::Object(serde_json::Map::new()),
            details: Vec::new(),
            message_flags: 0,
        }
    }
}

fn read_tag(xml: &str, tag: &str) -> anyhow::Result<String> {
    let re = Regex::new(&format!("<{}>([^<]+)</{}>", tag, tag))?;
    re.captures(xml)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| anyhow::anyhow!("Tag <{}> not found", tag))
}

fn parse_cover(json: &Value) -> Option<String> {
    if let Some(og) = json.get("ogImage").and_then(|v| v.as_str()) {
        return Some(format_cover(og));
    }
    if let Some(uri) = json.get("coverUri").and_then(|v| v.as_str()) {
        return Some(format_cover(uri));
    }
    if let Some(cover) = json.get("cover") {
        if let Some(uri) = cover.get("uri").and_then(|v| v.as_str()) {
            return Some(format_cover(uri));
        }
        if let Some(items) = cover.get("itemsUri").and_then(|v| v.as_array()) {
            if let Some(first) = items.first().and_then(|v| v.as_str()) {
                return Some(format_cover(first));
            }
        }
    }
    None
}

fn format_cover(uri: &str) -> String {
    format!("https://{}", uri.replace("%%", "400x400"))
}

#[async_trait]
impl SourceProvider for YandexMusicSource {
    fn name(&self) -> &'static str {
        "yandexmusic"
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["ymsearch", "ymrec"]
    }

    async fn search(&self, query: &str, search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        if let Some(st) = search_type {
            if st == "ymrec" {
                return self.recommendations_inner(query).await;
            }
        }

        if self.get_token().is_none() {
            return Ok(SourceResult::Error("Yandex Music access token is required for search.".into()));
        }

        let limit = 10; // default max search results

        let data = match self.api_raw("/search", vec![("text", query), ("type", "all"), ("page", "0")]).await {
            Ok(d) => d,
            Err(e) => return Ok(SourceResult::Error(format!("Search failed: {}", e))),
        };

        match search_type {
            Some("album") => {
                let items = data.get("albums").and_then(|a| a.get("results")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let results: Vec<TrackData> = items.into_iter()
                    .filter(|item| self.allow_unavail || item.get("available").and_then(|v| v.as_bool()).unwrap_or(true))
                    .take(limit)
                    .filter_map(|item| {
                        let id = item.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default();
                        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("Unknown Album").to_string();
                        let author = item.get("artists").and_then(|a| a.as_array())
                            .map(|arr| arr.iter().filter_map(|a| a.get("name").and_then(|n| n.as_str())).collect::<Vec<_>>().join(", "))
                            .unwrap_or_else(|| "Unknown Artist".into());
                        let artwork = parse_cover(&item);
                        Some(Self::build_search_result(&title, &author, &id, &format!("https://music.yandex.com/album/{}", id), artwork, "album"))
                    })
                    .collect();
                if results.is_empty() { Ok(SourceResult::Empty) } else { Ok(SourceResult::Search { data: results }) }
            }
            Some("artist") => {
                let items = data.get("artists").and_then(|a| a.get("results")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let results: Vec<TrackData> = items.into_iter()
                    .filter(|item| self.allow_unavail || item.get("available").and_then(|v| v.as_bool()).unwrap_or(true))
                    .take(limit)
                    .filter_map(|item| {
                        let id = item.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default();
                        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown Artist").to_string();
                        let artwork = parse_cover(&item);
                        Some(Self::build_search_result(&name, "Yandex Music", &id, &format!("https://music.yandex.com/artist/{}", id), artwork, "artist"))
                    })
                    .collect();
                if results.is_empty() { Ok(SourceResult::Empty) } else { Ok(SourceResult::Search { data: results }) }
            }
            Some("playlist") => {
                let items = data.get("playlists").and_then(|a| a.get("results")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let results: Vec<TrackData> = items.into_iter()
                    .take(limit)
                    .filter_map(|item| {
                        let kind = item.get("kind").and_then(|v| v.as_i64()).unwrap_or(0);
                        let id = kind.to_string();
                        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("Yandex Music Playlist").to_string();
                        let owner_name = item.get("owner")
                            .and_then(|o| o.get("name").or_else(|| o.get("login")).and_then(|v| v.as_str()))
                            .unwrap_or("Unknown");
                        let login = item.get("owner").and_then(|o| o.get("login").and_then(|v| v.as_str())).unwrap_or("unknown");
                        let artwork = parse_cover(&item);
                        Some(Self::build_search_result(&title, owner_name, &id, &format!("https://music.yandex.com/users/{}/playlists/{}", login, kind), artwork, "playlist"))
                    })
                    .collect();
                if results.is_empty() { Ok(SourceResult::Empty) } else { Ok(SourceResult::Search { data: results }) }
            }
            _ => {
                let tracks = data.get("tracks").and_then(|t| t.get("results")).cloned().unwrap_or(Value::Null);
                let tracks = Self::parse_tracks(&tracks, "com", self.allow_unavail);
                let tracks: Vec<TrackData> = tracks.into_iter().take(limit).collect();
                if tracks.is_empty() { Ok(SourceResult::Empty) } else { Ok(SourceResult::Search { data: tracks }) }
            }
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let clean = query.split(&['?', '#'][..]).next().unwrap_or(query);

        if self.get_token().is_none() {
            return Ok(SourceResult::Error("Yandex Music token required for resolution.".into()));
        }

        let ps = Self::patterns();

        // Pattern 0: /{artist|album|track}/{id}(/track/{id2})?
        if let Some(caps) = ps[0].captures(clean) {
            let domain = caps.get(1).map(|m| m.as_str()).unwrap_or("com");
            let type1 = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let id1 = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let id2 = caps.get(4).map(|m| m.as_str());

            if !id1.is_empty() {
                if type1 == "album" && id2.is_some() {
                    if let Some(tid) = id2 {
                        return self.resolve_track(tid, domain).await;
                    }
                }
                match type1 {
                    "album" => return self.resolve_album(id1, domain).await,
                    "artist" => return self.resolve_artist(id1, domain).await,
                    "track" => return self.resolve_track(id1, domain).await,
                    _ => {}
                }
            }
        }

        // Pattern 1: /users/{user}/playlists/{id}
        if let Some(caps) = ps[1].captures(clean) {
            let domain = caps.get(1).map(|m| m.as_str()).unwrap_or("com");
            let user = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let id = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            if !user.is_empty() && !id.is_empty() {
                return self.resolve_playlist(user, id, domain).await;
            }
        }

        // Pattern 2: /playlists/{uuid}
        if let Some(caps) = ps[2].captures(clean) {
            let domain = caps.get(1).map(|m| m.as_str()).unwrap_or("com");
            let uuid = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if !uuid.is_empty() {
                return self.resolve_playlist_uuid(uuid, domain).await;
            }
        }

        Ok(SourceResult::Empty)
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        if self.get_token().is_none() {
            return Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: serde_json::Value::Null,
                new_track: None,
                additional_data: serde_json::Value::Object(serde_json::Map::new()),
                exception: Some("Yandex Music token required for stream resolution.".into()),
            });
        }

        match self.get_download_url(&track.identifier).await {
            Ok(url) => Ok(TrackUrlResult {
                url: Some(url),
                protocol: Some("https".into()),
                format: serde_json::json!("mp3"),
                new_track: None,
                additional_data: serde_json::Value::Object(serde_json::Map::new()),
                exception: None,
            }),
            Err(e) => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: serde_json::Value::Null,
                new_track: None,
                additional_data: serde_json::Value::Object(serde_json::Map::new()),
                exception: Some(format!("Yandex stream URL failed: {}", e)),
            }),
        }
    }
}
