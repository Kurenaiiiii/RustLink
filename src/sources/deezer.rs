use async_trait::async_trait;
use md5::{Digest, Md5};
use reqwest::{header, Client};
use serde_json::Value;
use tracing::warn;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const PUBLIC_API: &str = "https://api.deezer.com";
const GW_URL: &str = "https://www.deezer.com/ajax/gw-light.php";

pub struct DeezerSource {
    client: Client,
    arl: Option<String>,
}

impl DeezerSource {
    pub fn new(arl: Option<String>) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            header::HeaderValue::from_static("https://www.deezer.com"),
        );
        headers.insert(
            header::REFERER,
            header::HeaderValue::from_static("https://www.deezer.com/"),
        );
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"),
        );

        let client = if let Ok(c) = Client::builder()
            .default_headers(headers)
            .build()
        {
            c
        } else {
            Client::new()
        };

        Self { client, arl }
    }

    fn has_arl(&self) -> bool {
        self.arl.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    async fn public_get(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(format!("{PUBLIC_API}{path}"))
            .send()
            .await?;
        Ok(resp.error_for_status()?.json().await?)
    }

    async fn gw_call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let resp = self
            .client
            .post(GW_URL)
            .query(&[("api_version", "1.0"), ("api_token", "null"), ("input", "3"), ("method", method)])
            .json(&params)
            .send()
            .await?;
        let data: Value = resp.error_for_status()?.json().await?;
        Ok(data.get("results").cloned().unwrap_or(data))
    }

    async fn login(&self) -> anyhow::Result<Value> {
        let _arl = match self.arl.as_ref() {
            Some(a) => a,
            None => anyhow::bail!("No ARL configured"),
        };
        self.gw_call("deezer.getUserData", serde_json::json!({})).await
    }

    async fn get_track_token(&self, sng_id: u64) -> anyhow::Result<String> {
        let result = self.gw_call(
            "deezer.pageTrack",
            serde_json::json!({ "SNG_ID": sng_id }),
        ).await?;
        result
            .get("DATA")
            .and_then(|d| d.get("TRACK_TOKEN"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("No TRACK_TOKEN found"))
    }

    fn build_download_url(
        md5_origin: &str,
        sng_id: u64,
        media_version: u64,
    ) -> Option<String> {
        let key = format!("{md5_origin}¤#@$¤!%$¤#@$¤!%$¤{media_version}");
        let hash = format!("{:x}", Md5::digest(key.as_bytes()));
        let partial = format!("{md5_origin}/audio-1-{hash}-0-{sng_id}-0-{media_version}-0-{hash}");
        let url = format!("https://e-cdns-proxy-{}.dzcdn.net/mobile/1/{}", 
            (sng_id % 10) + 1,
            partial,
        );
        Some(url)
    }

    fn parse_track(item: &Value, fallback_preview: Option<&str>) -> Option<TrackData> {
        let id = item.get("id").and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))?;
        let title = item.get("title").and_then(|v| v.as_str())?;
        let author = item
            .get("artist")
            .and_then(|a| a.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let duration = item.get("duration").and_then(|v| v.as_i64()).unwrap_or(0) * 1000;
        let uri = item
            .get("link")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let artwork = item
            .get("album")
            .and_then(|a| a.get("cover_big").or_else(|| a.get("cover_medium")).or_else(|| a.get("cover")))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let preview = item.get("preview").and_then(|v| v.as_str()).map(|s| s.to_string());
        let isrc = item.get("isrc").and_then(|v| v.as_str()).map(|s| s.to_string());

        let track_info = TrackInfo {
            identifier: id.to_string(),
            is_seekable: true,
            author: author.to_string(),
            length: duration,
            is_stream: false,
            position: 0,
            title: title.to_string(),
            uri,
            artwork_url: artwork,
            isrc,
            source_name: "deezer".to_string(),
            chapters: None,
        };

        let mut plugin_info = serde_json::json!({});
        if let Some(p) = preview.or_else(|| fallback_preview.map(|s| s.to_string())) {
            plugin_info["preview"] = serde_json::json!(p);
        }

        let mut track_data = TrackData {
            encoded: None,
            info: track_info,
            plugin_info,
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track_data.encoded = Some(encode_track(&track_data));
        Some(track_data)
    }

    async fn resolve_url(&self, url: &str) -> anyhow::Result<SourceResult> {
        // Normalize deezer.com URLs to the API path
        let path = if url.contains("/track/") {
            let id = url.rsplit('/').next().unwrap_or("");
            format!("/track/{id}")
        } else if url.contains("/album/") {
            let id = url.rsplit('/').next().unwrap_or("");
            format!("/album/{id}")
        } else if url.contains("/playlist/") {
            let id = url.rsplit('/').next().unwrap_or("");
            format!("/playlist/{id}")
        } else if url.contains("/artist/") {
            let id = url.rsplit('/').next().unwrap_or("");
            format!("/artist/{id}")
        } else {
            return Ok(SourceResult::Empty);
        };

        match self.public_get(&path).await {
            Ok(json) => {
                let kind = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match kind {
                    "track" => Ok(Self::parse_track(&json, None)
                        .map(SourceResult::Track)
                        .unwrap_or(SourceResult::Empty)),
                    "album" | "playlist" => {
                        let name = json.get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("Unknown");
                        let tracks = json.get("tracks")
                            .and_then(|t| t.get("data"))
                            .and_then(|d| d.as_array())
                            .or_else(|| json.get("tracks").and_then(|t| t.as_array()))
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| {
                                        let preview = t.get("preview").and_then(|p| p.as_str());
                                        Self::parse_track(t, preview)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();

                        if tracks.is_empty() {
                            return Ok(SourceResult::Empty);
                        }

                        let encoded = tracks[0].encoded.clone().unwrap_or_default();
                        Ok(SourceResult::Playlist {
                            data: crate::sources::PlaylistData {
                                encoded,
                                info: crate::sources::PlaylistInfo {
                                    name: name.to_string(),
                                    selected_track: 0,
                                },
                                plugin_info: serde_json::json!({}),
                                tracks,
                            },
                        })
                    }
                    "artist" => {
                        // Get top tracks for artist
                        let artist_id = json.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
                        let top = self.public_get(&format!("/artist/{artist_id}/top?limit=10")).await.ok();
                        let tracks = top
                            .and_then(|t| t.get("data").and_then(|d| d.as_array()).cloned())
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|t| Self::parse_track(t, None))
                            .collect::<Vec<_>>();

                        if tracks.is_empty() {
                            return Ok(SourceResult::Empty);
                        }

                        let encoded = tracks[0].encoded.clone().unwrap_or_default();
                        Ok(SourceResult::Playlist {
                            data: crate::sources::PlaylistData {
                                encoded,
                                info: crate::sources::PlaylistInfo {
                                    name: json.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown Artist").to_string(),
                                    selected_track: 0,
                                },
                                plugin_info: serde_json::json!({}),
                                tracks,
                            },
                        })
                    }
                    _ => Ok(SourceResult::Empty),
                }
            }
            Err(e) => {
                warn!(target: "Deezer", "Resolve error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }
}

#[async_trait]
impl SourceProvider for DeezerSource {
    fn name(&self) -> &'static str {
        "deezer"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["dz", "deezer"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["dzsearch", "deezersearch", "deezer"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let data = self
            .public_get(&format!("/search/track?q={}&limit=10", urlencoding(query)))
            .await;

        match data {
            Ok(json) => {
                let tracks: Vec<TrackData> = json
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| Self::parse_track(t, None))
                            .collect()
                    })
                    .unwrap_or_default();

                if tracks.is_empty() {
                    Ok(SourceResult::Empty)
                } else {
                    Ok(SourceResult::Search { data: tracks })
                }
            }
            Err(e) => {
                warn!(target: "Deezer", "Search error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let trimmed = query.trim();

        if trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("deezer:")
        {
            let url = if trimmed.starts_with("deezer:") {
                format!("https://www.deezer.com/{}", &trimmed["deezer:".len()..])
            } else {
                trimmed.to_string()
            };
            return self.resolve_url(&url).await;
        }

        // Treat as search
        self.search(trimmed, None).await
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let sng_id = track.identifier.parse::<u64>().unwrap_or(0);

        // Try ARL-based full track extraction
        if self.has_arl() && sng_id > 0 {
            if let Err(e) = self.login().await {
                warn!(target: "Deezer", "ARL login failed: {e}");
            } else {
                let track_result = self.gw_call(
                    "song.getListData",
                    serde_json::json!({ "SNG_IDS": [sng_id] }),
                ).await;

                if let Ok(data) = track_result {
                    if let Some(song) = data.as_array().and_then(|a| a.first()) {
                        let md5 = song.get("MD5_ORIGIN").and_then(|m| m.as_str());
                        let media_version = song.get("MEDIA_VERSION").and_then(|m| m.as_i64());

                        if let (Some(md5), Some(med_ver)) = (md5, media_version) {
                            if let Some(url) = Self::build_download_url(md5, sng_id, med_ver as u64) {
                                // Get track token for authentication
                                let token = self.get_track_token(sng_id).await.unwrap_or_default();
                                let final_url = if !token.is_empty() {
                                    format!("{url}?token={token}")
                                } else {
                                    url
                                };

                                return Ok(TrackUrlResult {
                                    url: Some(final_url),
                                    protocol: Some("http".into()),
                                    format: serde_json::json!({"protocol": "http", "quality": "MP3_128"}),
                                    new_track: None,
                                    additional_data: serde_json::json!({}),
                                    exception: None,
                                });
                            }
                        }
                    }
                }

                // Try alternative: song.getData
                let track_data = self.gw_call(
                    "song.getData",
                    serde_json::json!({ "SNG_ID": sng_id }),
                ).await;

                if let Ok(data) = track_data {
                    let md5 = data.get("MD5_ORIGIN").and_then(|m| m.as_str());
                    let media_version = data.get("MEDIA_VERSION").and_then(|m| m.as_i64());

                    if let (Some(md5), Some(med_ver)) = (md5, media_version) {
                        if let Some(url) = Self::build_download_url(md5, sng_id, med_ver as u64) {
                            let token = self.get_track_token(sng_id).await.unwrap_or_default();
                            let final_url = if !token.is_empty() {
                                format!("{url}?token={token}")
                            } else {
                                url
                            };

                            return Ok(TrackUrlResult {
                                url: Some(final_url),
                                protocol: Some("http".into()),
                                format: serde_json::json!({"protocol": "http", "quality": "MP3_128"}),
                                new_track: None,
                                additional_data: serde_json::json!({}),
                                exception: None,
                            });
                        }
                    }
                }
            }
        }

        // Fallback: use public API to get preview URL
        match self.public_get(&format!("/track/{sng_id}")).await {
            Ok(json) => {
                let preview = json.get("preview").and_then(|p| p.as_str());
                if let Some(url) = preview {
                    Ok(TrackUrlResult {
                        url: Some(url.to_string()),
                        protocol: Some("http".into()),
                        format: serde_json::json!({"protocol": "http", "quality": "preview"}),
                        new_track: None,
                        additional_data: serde_json::json!({}),
                        exception: if !self.has_arl() {
                            Some("Deezer: no ARL configured, returning 30s preview".into())
                        } else {
                            Some("Deezer: ARL extraction failed, returning 30s preview".into())
                        },
                    })
                } else {
                    Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: serde_json::json!({}),
                        new_track: None,
                        additional_data: serde_json::json!({}),
                        exception: Some("Deezer: no preview URL available".into()),
                    })
                }
            }
            Err(e) => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: serde_json::json!({}),
                new_track: None,
                additional_data: serde_json::json!({}),
                exception: Some(format!("Deezer: failed to fetch track: {e}")),
            }),
        }
    }
}

fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
