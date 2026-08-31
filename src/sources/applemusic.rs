use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const ITUNES_API: &str = "https://itunes.apple.com";
const DEFAULT_LIMIT: u32 = 10;

pub struct AppleMusicSource {
    client: Client,
}

impl AppleMusicSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    async fn search_api(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(format!("{ITUNES_API}{path}"))
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await?;
        Ok(resp.error_for_status()?.json().await?)
    }

    fn parse_track(item: &Value) -> Option<TrackData> {
        let id = item["trackId"].as_i64().or_else(|| item["trackId"].as_u64().map(|u| u as i64))?;
        let title = item["trackName"].as_str()?;
        let author = item["artistName"].as_str().unwrap_or("Unknown");
        let duration_ms = item["trackTimeMillis"].as_i64().unwrap_or(0);
        let artwork = item
            .get("artworkUrl100")
            .or_else(|| item.get("artworkUrl60"))
            .and_then(|u| u.as_str())
            .map(|s| s.replace("100x100bb", "600x600bb").replace("60x60bb", "600x600bb"));
        let uri = item["trackViewUrl"].as_str().map(|s| s.to_string());
        let preview = item["previewUrl"].as_str().map(|s| s.to_string());
        let isrc = item["isrc"].as_str().map(|s| s.to_string());
        let collection = item["collectionName"].as_str().unwrap_or("");

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: id.to_string(),
                is_seekable: true,
                author: author.to_string(),
                length: duration_ms,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri,
                artwork_url: artwork,
                isrc,
                source_name: "applemusic".to_string(),
                chapters: None,
            },
            plugin_info: if let Some(p) = preview {
                json!({ "preview": p, "collection": collection })
            } else {
                json!({ "collection": collection })
            },
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        Some(track)
    }

    fn extract_id(input: &str) -> Option<(String, String)> {
        // itunes:track:ID or itunes:album:ID
        if let Some(id) = input.strip_prefix("itunes:track:") {
            return Some(("track".into(), id.to_owned()));
        }
        if let Some(id) = input.strip_prefix("itunes:album:") {
            return Some(("album".into(), id.to_owned()));
        }
        if let Some(id) = input.strip_prefix("itunes:playlist:") {
            return Some(("playlist".into(), id.to_owned()));
        }

        // https://music.apple.com/{country}/album/{name}/{id}
        // https://music.apple.com/{country}/playlist/{name}/{id}
        if let Ok(url) = url::Url::parse(input) {
            if let Some(host) = url.host_str() {
                if host.contains("music.apple.com") || host.contains("itunes.apple.com") {
                    let segs: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
                    // Expected: [country, album|playlist|artist, name?, id]
                    for (i, seg) in segs.iter().enumerate() {
                        if *seg == "album" || *seg == "playlist" || *seg == "artist" {
                            let id = segs.get(i + 2).or_else(|| segs.get(i + 1))?;
                            return Some((seg.to_string(), id.to_string()));
                        }
                    }
                    // Fallback: check for /track/ or /song/
                    for (i, seg) in segs.iter().enumerate() {
                        if *seg == "track" || *seg == "song" {
                            let id = segs.get(i + 2).or_else(|| segs.get(i + 1))?;
                            return Some(("track".into(), id.to_string()));
                        }
                    }
                }
            }
        }
        None
    }

    async fn resolve_track(&self, id: &str) -> anyhow::Result<Option<TrackData>> {
        let data = self.search_api(&format!("/lookup?id={id}")).await?;
        let results = data["results"].as_array().map(|a| a.to_vec()).unwrap_or_default();
        let track = results.iter().find(|r| r["wrapperType"].as_str() == Some("track"));
        match track {
            Some(t) => Ok(Self::parse_track(t)),
            None => Ok(None),
        }
    }

    async fn resolve_album(&self, id: &str) -> anyhow::Result<Option<SourceResult>> {
        let data = self.search_api(&format!("/lookup?id={id}&entity=song")).await?;
        let results = data["results"].as_array().map(|a| a.to_vec()).unwrap_or_default();

        let album_info = results.iter().find(|r| r["wrapperType"].as_str() == Some("collection"));
        let album_name = album_info
            .and_then(|a| a["collectionName"].as_str())
            .unwrap_or("Apple Music Album");

        let tracks: Vec<TrackData> = results
            .iter()
            .filter(|r| r["wrapperType"].as_str() == Some("track"))
            .filter_map(Self::parse_track)
            .collect();

        if tracks.is_empty() {
            return Ok(None);
        }

        let encoded = tracks[0].encoded.clone().unwrap_or_default();
        Ok(Some(SourceResult::Playlist {
            data: crate::sources::PlaylistData {
                encoded,
                info: crate::sources::PlaylistInfo {
                    name: album_name.to_string(),
                    selected_track: 0,
                },
                plugin_info: json!({}),
                tracks,
            },
        }))
    }
}

#[async_trait]
impl SourceProvider for AppleMusicSource {
    fn name(&self) -> &'static str {
        "applemusic"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["am", "apple", "itunes"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["amsearch", "applemusic", "itunes"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let data = self
            .search_api(&format!("/search?term={encoded}&entity=song&limit={DEFAULT_LIMIT}"))
            .await;

        match data {
            Ok(json) => {
                let tracks: Vec<TrackData> = json
                    .get("results")
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(Self::parse_track).collect())
                    .unwrap_or_default();

                if tracks.is_empty() {
                    Ok(SourceResult::Empty)
                } else {
                    Ok(SourceResult::Search { data: tracks })
                }
            }
            Err(e) => {
                warn!(target: "AppleMusic", "Search error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let trimmed = query.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with("itunes:") {
            let (kind, id) = match Self::extract_id(trimmed) {
                Some(v) => v,
                None => return Ok(SourceResult::Empty),
            };

            info!(target: "AppleMusic", "Resolving {kind}: {id}");

            return match kind.as_str() {
                "track" | "song" => match self.resolve_track(&id).await {
                    Ok(Some(track)) => Ok(SourceResult::Track(track)),
                    Ok(None) => Ok(SourceResult::Empty),
                    Err(e) => Ok(SourceResult::Error(format!("Apple Music error: {e}"))),
                },
                "album" => match self.resolve_album(&id).await {
                    Ok(Some(result)) => Ok(result),
                    Ok(None) => Ok(SourceResult::Empty),
                    Err(e) => Ok(SourceResult::Error(format!("Apple Music error: {e}"))),
                },
                "playlist" => {
                    // iTunes API doesn't support playlist lookup; search instead
                    self.search(&id, None).await
                }
                _ => Ok(SourceResult::Empty),
            };
        }

        // Treat bare text as search
        self.search(trimmed, None).await
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        // Try to get a preview URL from the iTunes API
        match self.resolve_track(&track.identifier).await {
            Ok(Some(td)) => {
                let preview = td.plugin_info.get("preview").and_then(|p| p.as_str());
                if let Some(url) = preview {
                    Ok(TrackUrlResult {
                        url: Some(url.to_string()),
                        protocol: Some("https".into()),
                        format: json!({"protocol": "https", "quality": "aac-preview"}),
                        new_track: None,
                        additional_data: json!({}),
                        exception: Some("Apple Music: returning 30s AAC preview (no DRM-free audio available)".into()),
                    })
                } else {
                    Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: json!({}),
                        new_track: None,
                        additional_data: json!({}),
                        exception: Some("Apple Music: no preview URL available for this track".into()),
                    })
                }
            }
            Ok(None) => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: json!({}),
                new_track: None,
                additional_data: json!({}),
                exception: Some("Apple Music: track not found".into()),
            }),
            Err(e) => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: json!({}),
                new_track: None,
                additional_data: json!({}),
                exception: Some(format!("Apple Music: lookup failed: {e}")),
            }),
        }
    }
}
