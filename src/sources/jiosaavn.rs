use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use crate::decrypters::des_ecb::des_ecb_decrypt_base64;
use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const API_BASE: &str = "https://www.jiosaavn.com/api.php";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36";
const J_BUFFER: [u8; 8] = [51, 56, 51, 52, 54, 53, 57, 49];

pub struct JioSaavnSource {
    client: Client,
}

impl JioSaavnSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn pattern() -> Regex {
        Regex::new(r"https?://(?:www\.)?jiosaavn\.com/(?:(?P<type>album|featured|song|s/playlist|artist)/)(?:[^/]+/)(?P<id>[A-Za-z0-9_,-]+)").unwrap()
    }

    fn search_aliases() -> &'static [&'static str] {
        &["jssearch"]
    }

    fn rec_aliases() -> &'static [&'static str] {
        &["jsrec"]
    }

    async fn api_call(&self, params: Vec<(&str, &str)>) -> anyhow::Result<Value> {
        let url = reqwest::Url::parse_with_params(API_BASE, &params)?;
        let resp = self
            .client
            .get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("JioSaavn API request failed: {}", resp.status());
        }

        let text = resp.text().await?;
        let json: Value = serde_json::from_str(&text)?;
        Ok(json)
    }

    async fn fetch_song_metadata(&self, id: &str) -> anyhow::Result<Option<Value>> {
        let data = self
            .api_call(vec![("__call", "song.getDetails"), ("pids", id)])
            .await?;

        if let Some(details) = data.as_object() {
            if let Some(song) = details.get(id) {
                if let Some(payload) = Self::parse_song_payload(song) {
                    return Ok(Some(payload));
                }
            }
            if let Some(songs) = details.get("songs").and_then(|s| s.as_array()) {
                if let Some(first) = songs.first() {
                    if let Some(payload) = Self::parse_song_payload(first) {
                        return Ok(Some(payload));
                    }
                }
            }
        }

        let data = self
            .api_call(vec![
                ("__call", "webapi.get"),
                ("api_version", "4"),
                ("token", id),
                ("type", "song"),
            ])
            .await?;

        if let Some(songs) = data.get("songs").and_then(|s| s.as_array()) {
            if let Some(first) = songs.first() {
                return Ok(Self::parse_song_payload(first));
            }
        }

        Ok(None)
    }

    fn parse_song_payload(value: &Value) -> Option<Value> {
        let obj = value.as_object()?;
        let has_id = obj.contains_key("id") || obj.contains_key("song") || obj.contains_key("title");
        if !has_id {
            return None;
        }
        Some(value.clone())
    }

    fn parse_track(value: &Value) -> Option<TrackData> {
        let obj = value.as_object()?;
        let id = obj
            .get("id")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .or_else(|| {
                obj.get("id")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.to_string())
            })?;

        let title = Self::clean_string(
            obj.get("title")
                .or_else(|| obj.get("song"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown"),
        );

        let uri = obj
            .get("perma_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let duration_secs = obj
            .get("more_info")
            .and_then(|m| m.get("duration"))
            .or_else(|| obj.get("duration"))
            .and_then(|v| v.as_str())
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0);

        let more_info = obj.get("more_info");

        let author = more_info
            .and_then(|m| m.get("artistMap"))
            .and_then(|a| {
                a.get("primary_artists")
                    .or_else(|| a.get("artists"))
                    .and_then(|p| p.as_array())
                    .map(|artists| {
                        artists
                            .iter()
                            .filter_map(|a| {
                                a.get("name")
                                    .and_then(|n| n.as_str())
                                    .map(Self::clean_string)
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
            })
            .or_else(|| {
                more_info
                    .and_then(|m| m.get("music").and_then(|v| v.as_str()))
                    .or_else(|| obj.get("primary_artists").and_then(|v| v.as_str()))
                    .or_else(|| obj.get("singers").and_then(|v| v.as_str()))
                    .map(|s| Self::clean_string(s))
            })
            .unwrap_or_else(|| "Unknown Artist".to_string());

        let artwork_url = obj
            .get("image")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.replace("150x150", "500x500"));

        let info = TrackInfo {
            identifier: id,
            is_seekable: true,
            author,
            length: duration_secs * 1000,
            is_stream: false,
            position: 0,
            title,
            uri,
            artwork_url,
            isrc: None,
            source_name: "jiosaavn".into(),
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

    fn clean_string(value: &str) -> String {
        value
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
    }

    fn decrypt_url(encrypted_url: &str) -> anyhow::Result<String> {
        des_ecb_decrypt_base64(encrypted_url, &J_BUFFER)
            .map_err(|e| anyhow::anyhow!("DES-ECB decryption failed: {}", e))
    }

    async fn get_recommendations_inner(&self, query: &str) -> anyhow::Result<SourceResult> {
        let id;
        let id_re = Regex::new(r"^[A-Za-z0-9_,-]+$").unwrap();
        if id_re.is_match(query) {
            id = query.to_string();
        } else {
            let search_res = self
                .search(query, Some("jssearch"))
                .await?;
            match search_res {
                SourceResult::Search { ref data } if !data.is_empty() => {
                    id = data[0].info.identifier.clone();
                }
                _ => return Ok(SourceResult::Empty),
            }
        }

        let encoded_id = urlencoding(&format!("[\"{}\"]", id));

        let station_data = self
            .api_call(vec![
                ("__call", "webradio.createEntityStation"),
                ("api_version", "4"),
                ("ctx", "android"),
                ("entity_id", &encoded_id),
                ("entity_type", "queue"),
            ])
            .await?;

        if let Some(station_id) = station_data.get("stationid").and_then(|v| v.as_str()) {
            let song_data = self
                .api_call(vec![
                    ("__call", "webradio.getSong"),
                    ("api_version", "4"),
                    ("ctx", "android"),
                    ("stationid", &urlencoding(station_id)),
                    ("k", "20"),
                ])
                .await?;

            if let Some(playlist) = Self::get_station_playlist(&song_data) {
                return Ok(playlist);
            }
        }

        let metadata = self.fetch_song_metadata(&id).await?;
        if let Some(ref meta) = metadata {
            if let Some(artist_id) = meta
                .get("more_info")
                .and_then(|m| m.get("artistMap"))
                .and_then(|a| a.get("primary_artists"))
                .and_then(|p| p.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("id"))
                .and_then(|v| v.as_str())
            {
                let alt_data = self
                    .api_call(vec![
                        ("__call", "search.artistOtherTopSongs"),
                        ("api_version", "4"),
                        ("ctx", "wap6dot0"),
                        ("artist_ids", &urlencoding(artist_id)),
                        ("song_id", &urlencoding(&id)),
                        ("language", "unknown"),
                    ])
                    .await?;

                if let Some(arr) = alt_data.as_array() {
                    let tracks: Vec<TrackData> = arr
                        .iter()
                        .filter_map(Self::parse_track)
                        .collect();

                    if !tracks.is_empty() {
                        return Ok(SourceResult::Playlist {
                            data: PlaylistData {
                                encoded: String::new(),
                                info: PlaylistInfo {
                                    name: "JioSaavn Recommendations".into(),
                                    selected_track: 0,
                                },
                                plugin_info: serde_json::json!({"type": "recommendations"}),
                                tracks,
                            },
                        });
                    }
                }
            }
        }

        Ok(SourceResult::Empty)
    }

    fn get_station_playlist(value: &Value) -> Option<SourceResult> {
        let obj = value.as_object()?;
        if obj.contains_key("error") {
            return None;
        }

        let tracks: Vec<TrackData> = obj
            .values()
            .filter_map(|v| v.as_object())
            .filter_map(|v| v.get("song"))
            .filter_map(Self::parse_track)
            .collect();

        if tracks.is_empty() {
            return None;
        }

        Some(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: "JioSaavn Recommendations".into(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "recommendations"}),
                tracks,
            },
        })
    }
}

#[async_trait]
impl SourceProvider for JioSaavnSource {
    fn name(&self) -> &'static str {
        "jiosaavn"
    }

    fn search_terms(&self) -> &'static [&'static str] {
        Self::search_aliases()
    }

    async fn search(&self, query: &str, search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        if let Some(st) = search_type {
            if Self::rec_aliases().contains(&st) {
                return self.get_recommendations_inner(query).await;
            }
        }

        let data = self
            .api_call(vec![
                ("__call", "search.getResults"),
                ("q", query),
                ("includeMetaTags", "1"),
            ])
            .await?;

        let results = data
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        if results.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let tracks: Vec<TrackData> = results
            .iter()
            .filter_map(Self::parse_track)
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Search { data: tracks })
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let re = Self::pattern();
        let caps = re.captures(query).ok_or_else(|| anyhow::anyhow!("No match"))?;
        let r#type = caps.name("type").map(|m| m.as_str()).unwrap_or("");
        let id = caps.name("id").map(|m| m.as_str()).unwrap_or("");

        if r#type.is_empty() || id.is_empty() {
            return Ok(SourceResult::Empty);
        }

        if r#type == "song" {
            if let Some(track_data) = self.fetch_song_metadata(id).await? {
                if let Some(track) = Self::parse_track(&track_data) {
                    return Ok(SourceResult::Track(track));
                }
            }
            return Ok(SourceResult::Empty);
        }

        let api_type = if r#type == "featured" || r#type == "s/playlist" {
            "playlist"
        } else {
            r#type
        };

        let mut params = vec![
            ("__call", "webapi.get"),
            ("api_version", "4"),
            ("token", id),
            ("type", api_type),
        ];

        if r#type == "artist" {
            params.push(("n_song", "20"));
        } else {
            params.push(("n", "50"));
        }

        let data = self.api_call(params).await?;

        let list = data
            .get("list")
            .and_then(|l| l.as_array())
            .or_else(|| data.get("topSongs").and_then(|t| t.as_array()))
            .cloned()
            .unwrap_or_default();

        if list.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let tracks: Vec<TrackData> = list
            .iter()
            .filter_map(Self::parse_track)
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let name = data
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("name").and_then(|v| v.as_str()))
            .map(Self::clean_string)
            .unwrap_or_else(|| "Unknown".to_string());

        let playlist_name = if r#type == "artist" {
            format!("{}'s Top Tracks", name)
        } else {
            name
        };

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: playlist_name,
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": r#type}),
                tracks,
            },
        })
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let track_data = self.fetch_song_metadata(&track.identifier).await?;

        let encrypted_url = match track_data {
            Some(ref data) => data
                .get("encrypted_media_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            None => None,
        };

        let encrypted_url = match encrypted_url {
            Some(url) if !url.is_empty() => url,
            _ => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: serde_json::Value::Null,
                    new_track: None,
                    additional_data: serde_json::Value::Object(serde_json::Map::new()),
                    exception: Some("No encrypted_media_url found".into()),
                });
            }
        };

        let mut playback_url = Self::decrypt_url(&encrypted_url)?;

        let is_320kbps = track_data
            .as_ref()
            .and_then(|d| d.get("320kbps"))
            .and_then(|v| {
                v.as_str()
                    .or_else(|| {
                        v.as_bool().map(|b| if b { "true" } else { "false" })
                    })
            })
            .unwrap_or("false");

        if is_320kbps == "true" {
            playback_url = playback_url.replace("_96.mp4", "_320.mp4");
        }

        Ok(TrackUrlResult {
            url: Some(playback_url),
            protocol: Some("https".into()),
            format: serde_json::json!("mp4"),
            new_track: None,
            additional_data: serde_json::Value::Object(serde_json::Map::new()),
            exception: None,
        })
    }
}

fn urlencoding(s: &str) -> String {
    urlencoding::encode(s).to_string()
}
