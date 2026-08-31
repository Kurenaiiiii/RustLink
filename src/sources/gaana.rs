use aes::cipher::{BlockDecrypt, KeyInit};
use aes::cipher::generic_array::GenericArray;
use async_trait::async_trait;
use base64::Engine;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const API_URL: &str = "https://gaana.com/apiv2";
const STREAM_URL_API: &str = "https://gaana.com/api/stream-url";
const HLS_BASE_URL: &str = "https://vodhlsgaana-ebw.akamaized.net/";



pub struct GaanaSource {
    client: Client,
}

impl GaanaSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn pattern() -> Regex {
        Regex::new(r"^@?(?:https?://)?(?:www\.)?gaana\.com/(?P<type>song|album|playlist|artist)/(?P<seokey>[\w-]+)").unwrap()
    }

    async fn get_json(&self, params: &[(&str, &str)], query: &str) -> Option<Value> {
        let url = format!("{}?{}", API_URL, urlencode_params(params));
        let resp = self.client
            .post(&url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json, text/plain, */*")
            .header("Origin", "https://gaana.com")
            .header("Referer", &format!("https://gaana.com/{}", query))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .ok()?;

        if resp.status() != 200 {
            return None;
        }

        resp.json().await.ok()
    }

    fn map_track(track: &Value) -> Option<TrackData> {
        let title = track.get("track_title").and_then(|t| t.as_str())
            .or_else(|| track.get("name").and_then(|t| t.as_str()))?;

        let duration = track.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0) as i64 * 1000;

        let author = if let Some(artist) = track.get("artist") {
            if let Some(arr) = artist.as_array() {
                arr.iter()
                    .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else if let Some(obj) = artist.as_object() {
                obj.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown Artist").to_string()
            } else {
                "Unknown Artist".to_string()
            }
        } else {
            "Unknown Artist".to_string()
        };

        let identifier = track.get("track_id").and_then(|i| i.as_str())
            .or_else(|| track.get("seokey").and_then(|s| s.as_str()))?
            .to_string();

        let seokey = track.get("seokey").and_then(|s| s.as_str()).unwrap_or("");
        let uri = if seokey.is_empty() { None } else { Some(format!("https://gaana.com/song/{}", seokey)) };

        let info = TrackInfo {
            identifier,
            is_seekable: true,
            author,
            length: duration,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri,
            artwork_url: track.get("artwork_large").and_then(|a| a.as_str())
                .or_else(|| track.get("atw").and_then(|a| a.as_str()))
                .map(|s| s.to_string()),
            isrc: track.get("isrc").and_then(|i| i.as_str()).map(|s| s.to_string()),
            source_name: "gaana".into(),
            chapters: None,
        };

        let mut td = TrackData {
            encoded: None,
            info,
            plugin_info: serde_json::json!({
                "trackId": track.get("track_id").and_then(|i| i.as_str()),
                "albumName": track.get("album_title").and_then(|a| a.as_str()),
            }),
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        td.encoded = Some(encode_track(&td));
        Some(td)
    }

    fn decrypt_stream_path(encrypted: &str) -> Option<String> {
        let first_char = encrypted.chars().next()?;
        let offset = first_char.to_digit(10)? as usize;

        let b64 = &encrypted[offset + 16..];
        let padded = format!("{}==", b64);
        let ciphertext = base64::engine::general_purpose::STANDARD.decode(&padded).ok()?;
        if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
            return None;
        }

        let key = GenericArray::from_slice(b"gy1t#b@jl(b$wtme");
        let cipher = aes::Aes128::new(key);

        let mut prev = *b"xC4dmVJAq14BfntX";
        let mut decrypted = Vec::with_capacity(ciphertext.len());
        for chunk in ciphertext.chunks(16) {
            let mut block = GenericArray::clone_from_slice(chunk);
            cipher.decrypt_block(&mut block);
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            prev.copy_from_slice(chunk);
            decrypted.extend_from_slice(&block);
        }

        let raw = String::from_utf8_lossy(&decrypted)
            .trim_end_matches('\0')
            .chars()
            .filter(|&c| c as u8 >= 32 && c as u8 <= 126)
            .collect::<String>();

        if let Some(pos) = raw.find("/hls/") {
            let path = &raw[pos..];
            Some(format!("{}{}", HLS_BASE_URL.trim_end_matches('/'), path))
        } else {
            None
        }
    }
}

#[async_trait]
impl SourceProvider for GaanaSource {
    fn name(&self) -> &'static str {
        "gaana"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["gn"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["gnsearch", "gaanasearch"]
    }

    async fn search(&self, query: &str, search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let mut params = vec![
            ("country", "IN"),
            ("page", "0"),
            ("type", "search"),
            ("keyword", query),
        ];

        let st = search_type.unwrap_or("track");
        match st {
            "track" => params.push(("secType", "track")),
            "album" => params.push(("secType", "album")),
            "artist" => params.push(("secType", "artist")),
            "playlist" => params.push(("secType", "playlist")),
            _ => {}
        }

        let data = match self.get_json(&params, &format!("search/{}", urlencode(query))).await {
            Some(d) => d,
            None => return Ok(SourceResult::Empty),
        };

        let groups = data.get("gr").and_then(|g| g.as_array()).cloned().unwrap_or_default();
        if groups.is_empty() {
            return Ok(SourceResult::Empty);
        }

        let target = match st {
            "track" => "Track",
            "album" => "Album",
            "artist" => "Artist",
            "playlist" => "Playlist",
            _ => "Track",
        };

        let group = groups.iter().find(|g| {
            g.get("ty").and_then(|t| t.as_str()) == Some(target)
        });

        let items = group.and_then(|g| g.get("gd").and_then(|gd| gd.as_array()).cloned()).unwrap_or_default();
        if items.is_empty() {
            return Ok(SourceResult::Empty);
        }

        if st == "track" {
            let ids: Vec<String> = items.iter().filter_map(|item| {
                item.get("seo").and_then(|s| s.as_str())
                    .or_else(|| item.get("id").and_then(|i| i.as_str()))
                    .map(|s| s.to_string())
            }).collect();

            let promises: Vec<_> = ids.into_iter().map(|id| {
                async move {
                    self.get_song(&id).await
                }
            }).collect();

            let tracks: Vec<TrackData> = futures::future::join_all(promises)
                .await
                .into_iter()
                .filter_map(|r| r)
                .collect();

            return if tracks.is_empty() {
                Ok(SourceResult::Empty)
            } else {
                Ok(SourceResult::Search { data: tracks })
            };
        }

        let tracks: Vec<TrackData> = items.into_iter().filter_map(|item| {
            let title = item.get("ti").and_then(|t| t.as_str())
                .or_else(|| item.get("name").and_then(|n| n.as_str()))
                .unwrap_or("Unknown");
            let seokey = item.get("seo").and_then(|s| s.as_str()).unwrap_or("");

            let info = TrackInfo {
                identifier: seokey.to_string(),
                is_seekable: true,
                author: item.get("sti").and_then(|s| s.as_str()).unwrap_or("Gaana").to_string(),
                length: 0,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: if seokey.is_empty() { None } else { Some(format!("https://gaana.com/{}/{}", st, seokey)) },
                artwork_url: item.get("aw").and_then(|a| a.as_str())
                    .or_else(|| item.get("atw").and_then(|a| a.as_str()))
                    .map(|s| s.to_string()),
                isrc: None,
                source_name: "gaana".into(),
                chapters: None,
            };
            let mut td = TrackData {
                encoded: None,
                info,
                plugin_info: serde_json::json!({"type": st}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            td.encoded = Some(encode_track(&td));
            Some(td)
        }).collect();

        if tracks.is_empty() {
            Ok(SourceResult::Empty)
        } else {
            Ok(SourceResult::Search { data: tracks })
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let caps = match Self::pattern().captures(query) {
            Some(c) => c,
            None => return Ok(SourceResult::Empty),
        };

        let typ = caps.name("type").map(|m| m.as_str()).unwrap_or("");
        let seokey = caps.name("seokey").map(|m| m.as_str()).unwrap_or("");

        let result = match typ {
            "song" => self.get_song(seokey).await.map(SourceResult::Track),
            "album" => self.get_album(seokey).await,
            "playlist" => self.get_playlist(seokey).await,
            "artist" => self.get_artist(seokey).await,
            _ => None,
        };

        Ok(result.unwrap_or(SourceResult::Empty))
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let track_id = &track.identifier;
        if track_id.parse::<u64>().is_ok() {
            let quality = "high";
            let body = format!("quality={}&track_id={}&stream_format=mp4", quality, track_id);

            let resp = self.client
                .post(STREAM_URL_API)
                .header("User-Agent", USER_AGENT)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("Origin", "https://gaana.com")
                .header("Referer", "https://gaana.com/")
                .body(body)
                .send()
                .await?;

            if resp.status() == 200 {
                if let Ok(data) = resp.json::<Value>().await {
                    if data.get("api_status").and_then(|s| s.as_str()) == Some("success") {
                        let stream_path = data.get("data")
                            .and_then(|d| d.get("stream_path"))
                            .and_then(|s| s.as_str());

                        if let Some(path) = stream_path {
                            if let Some(hls_url) = Self::decrypt_stream_path(path) {
                                return Ok(TrackUrlResult {
                                    url: Some(hls_url),
                                    protocol: Some("hls".into()),
                                    format: serde_json::json!("mpegts"),
                                    new_track: None,
                                    additional_data: serde_json::json!({}),
                                    exception: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: serde_json::json!({}),
            new_track: None,
            additional_data: serde_json::json!({}),
            exception: Some("Gaana track could not be resolved. Use search-based mirror resolution.".into()),
        })
    }
}

impl GaanaSource {
    async fn get_song(&self, seokey: &str) -> Option<TrackData> {
        let data = self.get_json(&[("type", "songDetail"), ("seokey", seokey)], &format!("song/{}", seokey)).await?;
        let tracks = data.get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        let first = tracks.first()?;
        Self::map_track(first)
    }

    async fn get_album(&self, seokey: &str) -> Option<SourceResult> {
        let data = self.get_json(&[("type", "albumDetail"), ("seokey", seokey)], &format!("album/{}", seokey)).await?;
        let tracks = data.get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        if tracks.is_empty() {
            return None;
        }

        let album = data.get("album").and_then(|a| a.as_object()).cloned().unwrap_or_default();
        let album_title = album.get("title").and_then(|t| t.as_str()).unwrap_or("Unknown Album");
        let artwork = album.get("atw").and_then(|a| a.as_str()).map(|s| s.to_string());
        let artist = album.get("artist").and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());

        let playlist_tracks: Vec<TrackData> = tracks.iter().filter_map(|t| {
            let title = t.get("track_title").and_then(|s| s.as_str())
                .or_else(|| t.get("name").and_then(|s| s.as_str()))?;
            let tid = t.get("track_id").and_then(|i| i.as_str())
                .or_else(|| t.get("seokey").and_then(|s| s.as_str()))?
                .to_string();
            let dur = t.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0) as i64 * 1000;
            let t_seokey = t.get("seokey").and_then(|s| s.as_str()).unwrap_or("");

            let info = TrackInfo {
                identifier: tid,
                is_seekable: true,
                author: artist.clone().unwrap_or_else(|| "Unknown Artist".into()),
                length: dur,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: if t_seokey.is_empty() { None } else { Some(format!("https://gaana.com/song/{}", t_seokey)) },
                artwork_url: artwork.clone(),
                isrc: t.get("isrc").and_then(|i| i.as_str()).map(|s| s.to_string()),
                source_name: "gaana".into(),
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
            Some(td)
        }).collect();

        if playlist_tracks.is_empty() {
            return None;
        }

        let encoded = playlist_tracks.first().and_then(|t| t.encoded.clone()).unwrap_or_default();
        Some(SourceResult::Playlist {
            data: PlaylistData {
                encoded,
                info: PlaylistInfo {
                    name: album_title.to_string(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "album", "url": format!("https://gaana.com/album/{}", seokey)}),
                tracks: playlist_tracks,
            },
        })
    }

    async fn get_playlist(&self, seokey: &str) -> Option<SourceResult> {
        let data = self.get_json(&[("type", "playlistDetail"), ("seokey", seokey)], &format!("playlist/{}", seokey)).await?;
        let tracks = data.get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        if tracks.is_empty() {
            return None;
        }

        let playlist = data.get("playlist").and_then(|p| p.as_object()).cloned().unwrap_or_default();
        let name = playlist.get("title").and_then(|t| t.as_str()).unwrap_or("Unknown Playlist");
        let artwork = playlist.get("atw").and_then(|a| a.as_str()).map(|s| s.to_string());

        let playlist_tracks: Vec<TrackData> = tracks.iter().filter_map(|t| Self::map_track(t)).collect();
        if playlist_tracks.is_empty() {
            return None;
        }

        let encoded = playlist_tracks.first().and_then(|t| t.encoded.clone()).unwrap_or_default();
        Some(SourceResult::Playlist {
            data: PlaylistData {
                encoded,
                info: PlaylistInfo {
                    name: name.to_string(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "playlist", "url": format!("https://gaana.com/playlist/{}", seokey), "artwork_url": artwork}),
                tracks: playlist_tracks,
            },
        })
    }

    async fn get_artist(&self, seokey: &str) -> Option<SourceResult> {
        let data = self.get_json(&[("type", "artistDetail"), ("seokey", seokey)], &format!("artist/{}", seokey)).await?;
        let artists = data.get("artist").and_then(|a| a.as_array()).cloned().unwrap_or_default();
        let first = artists.first()?;
        let artist_id = first.get("artist_id").and_then(|i| i.as_str())?;
        let artist_name = first.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown Artist");
        let artwork = first.get("artwork_bio").and_then(|a| a.as_str()).map(|s| s.to_string());

        let tracks_data = self.get_json(&[
            ("type", "artistTrackList"),
            ("id", artist_id),
            ("language", ""),
            ("order", "0"),
            ("page", "0"),
            ("sortBy", "popularity"),
        ], &format!("artist/{}", seokey)).await?;

        let tracks = tracks_data.get("tracks").and_then(|t| t.as_array())
            .or_else(|| tracks_data.get("entities").and_then(|e| e.as_array()))
            .cloned()
            .unwrap_or_default();

        let playlist_tracks: Vec<TrackData> = tracks.iter().filter_map(|t| {
            let title = t.get("name").and_then(|n| n.as_str())?;
            let tid = t.get("entity_id").and_then(|i| i.as_str())?;
            let t_seokey = t.get("seokey").and_then(|s| s.as_str()).unwrap_or("");

            let entities = t.get("entity_info").and_then(|e| e.as_array()).cloned().unwrap_or_default();
            let get_val = |key: &str| -> Option<String> {
                entities.iter().find(|e| e.get("key").and_then(|k| k.as_str()) == Some(key))
                    .and_then(|e| e.get("value"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            };

            let duration = get_val("duration").and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0) as i64 * 1000;
            let artists_raw = get_val("artist");
            let author = artists_raw.as_ref().map(|a| {
                if let Ok(arr) = serde_json::from_str::<Value>(a) {
                    arr.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.get("name").and_then(|n| n.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }).unwrap_or_else(|| a.clone())
                } else {
                    a.clone()
                }
            }).unwrap_or_else(|| "Unknown Artist".into());

            let info = TrackInfo {
                identifier: tid.to_string(),
                is_seekable: true,
                author,
                length: duration,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: if t_seokey.is_empty() { None } else { Some(format!("https://gaana.com/song/{}", t_seokey)) },
                artwork_url: t.get("atw").and_then(|a| a.as_str()).or(artwork.as_deref()).map(|s| s.to_string()),
                isrc: get_val("isrc"),
                source_name: "gaana".into(),
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
            Some(td)
        }).collect();

        if playlist_tracks.is_empty() {
            return None;
        }

        let encoded = playlist_tracks.first().and_then(|t| t.encoded.clone()).unwrap_or_default();
        Some(SourceResult::Playlist {
            data: PlaylistData {
                encoded,
                info: PlaylistInfo {
                    name: format!("{}'s Top Tracks", artist_name),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "artist", "url": format!("https://gaana.com/artist/{}", seokey), "artwork_url": artwork}),
                tracks: playlist_tracks,
            },
        })
    }
}

fn urlencode_params(params: &[(&str, &str)]) -> String {
    params.iter()
        .map(|(k, v)| format!("{}={}", url::form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>(),
                                url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
