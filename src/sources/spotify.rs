use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

#[allow(dead_code)]
pub struct SpotifySource {
    client: Client,
    client_id: String,
    client_secret: String,
    market: String,
    token: Arc<RwLock<Option<TokenCache>>>,
}

struct TokenCache {
    access_token: String,
    expires_at: std::time::Instant,
}

impl SpotifySource {
    pub fn new(client_id: String, client_secret: String, market: String) -> Self {
        Self {
            client: Client::builder()
                .user_agent("RustLink/3.8.0")
                .build()
                .unwrap(),
            client_id,
            client_secret,
            market,
            token: Arc::new(RwLock::new(None)),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.client_id.is_empty() && !self.client_secret.is_empty()
    }

    async fn ensure_token(&self) -> anyhow::Result<String> {
        {
            let cached = self.token.read().await;
            if let Some(t) = cached.as_ref() {
                if t.expires_at > std::time::Instant::now() {
                    return Ok(t.access_token.clone());
                }
            }
        }

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
        ];

        let resp: Value = self
            .client
            .post("https://accounts.spotify.com/api/token")
            .form(&params)
            .send()
            .await?
            .json()
            .await?;

        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Spotify auth failed"))?
            .to_owned();
        let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);

        let mut cached = self.token.write().await;
        *cached = Some(TokenCache {
            access_token: token.clone(),
            expires_at: std::time::Instant::now()
                + std::time::Duration::from_secs(expires_in.saturating_sub(60)),
        });

        Ok(token)
    }

    async fn api_get(&self, path: &str) -> anyhow::Result<Value> {
        let token = self.ensure_token().await?;
        let url = format!("https://api.spotify.com/v1{path}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await?;
        Ok(resp.json().await?)
    }

    async fn resolve_track(&self, id: &str) -> anyhow::Result<Option<TrackData>> {
        let data = self.api_get(&format!("/tracks/{id}")).await?;

        let title = data["name"].as_str().unwrap_or("Unknown");
        let artists: Vec<&str> = data["artists"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let author = artists.join(", ");
        let duration_ms = data["duration_ms"].as_i64().unwrap_or(0);
        let album_art = data
            .pointer("/album/images/0/url")
            .and_then(|u| u.as_str());

        let uri = format!("https://open.spotify.com/track/{id}");

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: format!("spotify:{id}"),
                is_seekable: true,
                author,
                length: duration_ms,
                is_stream: false,
                position: 0,
                title: title.to_owned(),
                uri: Some(uri),
                artwork_url: album_art.map(|s| s.to_owned()),
                isrc: data["external_ids"]["isrc"].as_str().map(|s| s.to_owned()),
                source_name: "spotify".into(),
                chapters: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        Ok(Some(track))
    }

    async fn resolve_album(&self, id: &str) -> anyhow::Result<Option<SourceResult>> {
        let data = self.api_get(&format!("/albums/{id}")).await?;
        let name = data["name"].as_str().unwrap_or("Unknown Album");
        let items = data
            .pointer("/tracks/items")
            .and_then(|i| i.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let mut tracks = Vec::new();
        for item in &items {
            let tid = item["id"].as_str().unwrap_or("");
            if !tid.is_empty() {
                if let Some(track) = self.resolve_track(tid).await.unwrap_or(None) {
                    tracks.push(track);
                }
            }
        }

        if tracks.is_empty() {
            return Ok(None);
        }

        let encoded = tracks[0].encoded.clone().unwrap_or_default();
        Ok(Some(SourceResult::Playlist {
            data: crate::sources::PlaylistData {
                info: crate::sources::PlaylistInfo {
                    name: name.to_owned(),
                    selected_track: 0,
                },
                encoded,
                plugin_info: json!({}),
                tracks,
            },
        }))
    }

    async fn resolve_playlist(&self, id: &str) -> anyhow::Result<Option<SourceResult>> {
        let data = self.api_get(&format!("/playlists/{id}")).await?;
        let name = data["name"].as_str().unwrap_or("Unknown Playlist");
        let items = data
            .pointer("/tracks/items")
            .and_then(|i| i.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let mut tracks = Vec::new();
        for item in &items {
            let track_data = &item["track"];
            if track_data.is_null() {
                continue;
            }
            let tid = track_data["id"].as_str().unwrap_or("");
            if !tid.is_empty() {
                if let Some(track) = self.resolve_track(tid).await.unwrap_or(None) {
                    tracks.push(track);
                }
            }
        }

        if tracks.is_empty() {
            return Ok(None);
        }

        let encoded = tracks[0].encoded.clone().unwrap_or_default();
        Ok(Some(SourceResult::Playlist {
            data: crate::sources::PlaylistData {
                info: crate::sources::PlaylistInfo {
                    name: name.to_owned(),
                    selected_track: 0,
                },
                encoded,
                plugin_info: json!({}),
                tracks,
            },
        }))
    }
}

fn extract_spotify_id(input: &str) -> Option<(String, String)> {
    // Format: spotify:track:ID or https://open.spotify.com/track/ID
    if let Some(id) = input.strip_prefix("spotify:track:") {
        return Some(("track".into(), id.to_owned()));
    }
    if let Some(id) = input.strip_prefix("spotify:album:") {
        return Some(("album".into(), id.to_owned()));
    }
    if let Some(id) = input.strip_prefix("spotify:playlist:") {
        return Some(("playlist".into(), id.to_owned()));
    }
    if let Some(id) = input.strip_prefix("spotify:artist:") {
        return Some(("artist".into(), id.to_owned()));
    }

    if let Ok(url) = url::Url::parse(input) {
        if url.host_str() == Some("open.spotify.com") {
            let segs: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
            if segs.len() >= 2 {
                return Some((segs[0].to_owned(), segs[1].to_owned()));
            }
        }
    }

    None
}

#[async_trait]
impl SourceProvider for SpotifySource {
    fn name(&self) -> &'static str {
        "spotify"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["sp", "spotify"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["spsearch", "spotify", "spotifysearch"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        if !self.is_configured() {
            return Ok(SourceResult::Empty);
        }

        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let data = self.api_get(&format!("/search?q={encoded}&type=track&market={}&limit=10", self.market)).await;

        match data {
            Ok(json) => {
                let tracks: Vec<TrackData> = json
                    .pointer("/tracks/items")
                    .and_then(|i| i.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                let id = item["id"].as_str()?;
                                let title = item["name"].as_str().unwrap_or("Unknown");
                                let artists: Vec<&str> = item["artists"]
                                    .as_array()
                                    .map(|arr| arr.iter().filter_map(|a| a["name"].as_str()).collect())
                                    .unwrap_or_default();
                                let author = artists.join(", ");
                                let duration_ms = item["duration_ms"].as_i64().unwrap_or(0);
                                let album_art = item
                                    .pointer("/album/images/0/url")
                                    .and_then(|u| u.as_str());

                                let mut track = TrackData {
                                    encoded: None,
                                    info: TrackInfo {
                                        identifier: format!("spotify:{id}"),
                                        is_seekable: true,
                                        author,
                                        length: duration_ms,
                                        is_stream: false,
                                        position: 0,
                                        title: title.to_owned(),
                                        uri: Some(format!("https://open.spotify.com/track/{id}")),
                                        artwork_url: album_art.map(|s| s.to_owned()),
                                        isrc: item["external_ids"]["isrc"].as_str().map(|s| s.to_owned()),
                                        source_name: "spotify".into(),
                                        chapters: None,
                                    },
                                    plugin_info: json!({}),
                                    user_data: json!({}),
                                    details: Vec::new(),
                                    message_flags: 0,
                                };
                                track.encoded = Some(encode_track(&track));
                                Some(track)
                            })
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
                warn!(target: "Spotify", "Search error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        if !self.is_configured() {
            return Ok(SourceResult::Empty);
        }

        let (kind, id) = match extract_spotify_id(query) {
            Some(v) => v,
            None => return Ok(SourceResult::Empty),
        };

        info!(target: "Spotify", "Resolving {kind}: {id}");

        match kind.as_str() {
            "track" => match self.resolve_track(&id).await {
                Ok(Some(track)) => Ok(SourceResult::Track(track)),
                Ok(None) => Ok(SourceResult::Empty),
                Err(e) => Ok(SourceResult::Error(format!("Spotify error: {e}"))),
            },
            "album" => match self.resolve_album(&id).await {
                Ok(Some(result)) => Ok(result),
                Ok(None) => Ok(SourceResult::Empty),
                Err(e) => Ok(SourceResult::Error(format!("Spotify error: {e}"))),
            },
            "playlist" => match self.resolve_playlist(&id).await {
                Ok(Some(result)) => Ok(result),
                Ok(None) => Ok(SourceResult::Empty),
                Err(e) => Ok(SourceResult::Error(format!("Spotify error: {e}"))),
            },
            _ => Ok(SourceResult::Empty),
        }
    }

    async fn get_track_url(&self, _track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        warn!(
            target: "Spotify",
            "get_track_url called — no direct audio URL available for Spotify tracks"
        );
        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: json!({}),
            new_track: None,
            additional_data: json!({}),
            exception: Some("Spotify does not provide direct audio URLs. Use a fallback source.".into()),
        })
    }
}
