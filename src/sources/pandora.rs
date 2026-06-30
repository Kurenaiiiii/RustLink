use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const PANDORA_BASE: &str = "https://www.pandora.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

#[derive(Clone)]
struct PandoraAuth {
    auth_token: String,
    csrf_token: String,
}

pub struct PandoraSource {
    client: Client,
    csrf_token_config: Option<String>,
    remote_token_url: Option<String>,
    auth: std::sync::Mutex<Option<PandoraAuth>>,
}

impl PandoraSource {
    pub fn new(
        csrf_token: Option<String>,
        remote_token_url: Option<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            csrf_token_config: csrf_token,
            remote_token_url,
            auth: std::sync::Mutex::new(None),
        }
    }

    fn pattern() -> Regex {
        Regex::new(r"^https?://(?:www\.)?pandora\.com/(?:(playlist|station|podcast|artist))/.+")
            .unwrap()
    }

    fn headers(auth: &PandoraAuth) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("User-Agent", USER_AGENT.parse().unwrap());
        h.insert("Accept", "application/json".parse().unwrap());
        h.insert(
            "Content-Type",
            "application/json".parse().unwrap(),
        );
        h.insert("Cookie", auth.csrf_token.parse().unwrap());
        h.insert(
            "X-Auth-Token",
            auth.auth_token.parse().unwrap(),
        );
        h
    }

    fn get_auth(&self) -> Option<PandoraAuth> {
        self.auth.lock().unwrap().clone()
    }

    fn set_auth(&self, auth: PandoraAuth) {
        *self.auth.lock().unwrap() = Some(auth);
    }

    async fn ensure_auth(&self) -> anyhow::Result<PandoraAuth> {
        if let Some(ref a) = self.get_auth() {
            return Ok(a.clone());
        }

        if let Some(ref url) = self.remote_token_url {
            if let Ok(resp) = self.client.get(url).send().await {
                if let Ok(body) = resp.json::<Value>().await {
                    let at = body["authToken"].as_str().map(|s| s.to_string());
                    let ct = body
                        .get("csrfToken")
                        .or_else(|| body.get("csrfToken"))
                        .and_then(|v| v.as_str().map(|s| format!("csrftoken={};Path=/;Domain=.pandora.com;Secure", s)));
                    if let (Some(auth_token), Some(csrf_token)) = (at, ct) {
                        let a = PandoraAuth { auth_token, csrf_token };
                        self.set_auth(a.clone());
                        return Ok(a);
                    }
                }
            }
        }

        if let Some(ref ct) = self.csrf_token_config {
            let csrf_token = format!("csrftoken={};Path=/;Domain=.pandora.com;Secure", ct);
            let body = serde_json::json!({
                "authMethod": "anonymous",
                "androidAuth": null
            });
            let resp = self
                .client
                .post(format!("{}/api/v1/auth/login", PANDORA_BASE))
                .header("User-Agent", USER_AGENT)
                .header("Content-Type", "application/json")
                .header("Cookie", &csrf_token)
                .json(&body)
                .send()
                .await?;

            if resp.status().is_success() {
                if let Ok(body) = resp.json::<Value>().await {
                    if let Some(auth_token) = body["authToken"].as_str().map(|s| s.to_string()) {
                        let a = PandoraAuth { auth_token, csrf_token };
                        self.set_auth(a.clone());
                        return Ok(a);
                    }
                }
            }
        }

        anyhow::bail!("Pandora auth failed: no token URL, CSRF token, or anonymous login configured")
    }

    fn build_artwork_url(artwork: &Value) -> Option<String> {
        let art_id = artwork
            .get("artId")
            .or_else(|| artwork.get("url"))
            .or_else(|| artwork.get("artUrl"))
            .and_then(|v| v.as_str())?;
        if art_id.starts_with("http") {
            Some(art_id.to_string())
        } else {
            Some(format!("https://content-images.p-cdn.com/{}", art_id))
        }
    }

    fn parse_annotation(item: &Value) -> Option<TrackData> {
        let pandora_id = item
            .get("pandoraId")
            .or_else(|| item.get("id"))
            .and_then(|v| v.as_str())?;

        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown");
        let artist = item
            .get("artistName")
            .and_then(|v| {
                if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else if let Some(obj) = v.as_object() {
                    obj.get("name").and_then(|n| n.as_str().map(|s| s.to_string()))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Unknown Artist".to_string());

        let duration = item
            .get("duration")
            .or_else(|| item.get("trackLength"))
            .or_else(|| item.get("length"))
            .and_then(|v| v.as_f64())
            .map(|s| (s * 1000.0) as i64)
            .unwrap_or(-1);

        let uri_path = item
            .get("shareableUrlPath")
            .or_else(|| item.get("urlPath"))
            .and_then(|v| v.as_str());
        let uri = uri_path.map(|p| format!("https://www.pandora.com{}", p));

        let artwork = item
            .get("icon")
            .or_else(|| {
                item.get("art")
                    .and_then(|a| a.as_array())
                    .and_then(|arr| arr.first())
            })
            .and_then(Self::build_artwork_url);

        let info = TrackInfo {
            identifier: format!("pandora:{}", pandora_id),
            is_seekable: duration > 0,
            author: artist,
            length: duration,
            is_stream: false,
            position: 0,
            title: name.to_string(),
            uri,
            artwork_url: artwork,
            isrc: item.get("isrc").and_then(|v| v.as_str()).map(|s| s.to_string()),
            source_name: "pandora".into(),
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

    async fn api_post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let auth = self.ensure_auth().await?;
        let resp = self
            .client
            .post(format!("{}{}", PANDORA_BASE, path))
            .headers(Self::headers(&auth))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Pandora API error: {}", resp.status());
        }

        let text = resp.text().await?;
        Ok(serde_json::from_str(&text)?)
    }

    async fn search_inner(&self, query: &str) -> anyhow::Result<SourceResult> {
        self.ensure_auth().await?;

        let body = serde_json::json!({
            "searchInput": query,
            "enablePandoraModes": false,
            "enableNewSearchExperience": false
        });

        let data = self
            .api_post("/api/v1/search/top-hits", body)
            .await?;

        let annotations = data
            .get("annotations")
            .and_then(|a| a.as_object())
            .cloned()
            .unwrap_or_default();

        let results = data
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        let mut tracks: Vec<TrackData> = Vec::new();
        for item in &results {
            if let Some(track_id) = item.as_str() {
                if let Some(ann) = annotations.get(track_id) {
                    if let Some(track) = Self::parse_annotation(ann) {
                        tracks.push(track);
                    }
                }
            }
        }

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Search { data: tracks })
    }

    #[allow(dead_code)]
    async fn resolve_by_id(&self, id: &str) -> anyhow::Result<Option<TrackData>> {
        let body = serde_json::json!({
            "pandoraIds": [id]
        });
        let data = self.api_post("/api/v1/pandora/annotate", body).await?;
        let annotations = data.get("annotations").and_then(|a| a.as_object());
        if let Some(anns) = annotations {
            if let Some(ann) = anns.get(id) {
                return Ok(Self::parse_annotation(ann));
            }
        }
        Ok(None)
    }

    async fn resolve_playlist(&self, id: &str) -> anyhow::Result<SourceResult> {
        let body = serde_json::json!({
            "request": {
                "pandoraId": id,
                "playlistVersion": -1,
                "offset": 0,
                "limit": 50,
                "annotationLimit": 50,
                "allowedTypes": ["TR", "AL"],
                "bypassPrivacyRules": true
            }
        });
        let data = self.api_post("/api/v1/playlist/data", body).await?;
        let annotations = data.get("annotations").and_then(|a| a.as_object()).cloned().unwrap_or_default();
        let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("Playlist");

        let tracks: Vec<TrackData> = annotations
            .values()
            .filter_map(Self::parse_annotation)
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: name.to_string(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "playlist"}),
                tracks,
            },
        })
    }

    async fn resolve_station(&self, id: &str) -> anyhow::Result<SourceResult> {
        let body = serde_json::json!({
            "pandoraId": id,
            "playlistVersion": -1,
            "offset": 0,
            "limit": 50,
            "annotationLimit": 50,
            "allowedTypes": ["TR"],
            "bypassPrivacyRules": true
        });
        let data = self.api_post("/api/v1/station/data", body).await?;
        let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("Station");
        let seeds = data.get("seeds").and_then(|s| s.as_array()).cloned().unwrap_or_default();

        let mut tracks: Vec<TrackData> = Vec::new();
        for seed in &seeds {
            if let Some(song) = seed.get("song") {
                let pandora_id = song.get("songId").and_then(|v| v.as_str());
                if let Some(pid) = pandora_id {
                    let body2 = serde_json::json!({ "pandoraIds": [pid] });
                    if let Ok(ann_data) = self.api_post("/api/v1/pandora/annotate", body2).await {
                        if let Some(anns) = ann_data.get("annotations").and_then(|a| a.as_object()) {
                            if let Some(ann) = anns.get(pid) {
                                if let Some(track) = Self::parse_annotation(ann) {
                                    tracks.push(track);
                                }
                            }
                        }
                    }
                }
            }
        }

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: name.to_string(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "station"}),
                tracks,
            },
        })
    }

    async fn resolve_artist(&self, id: &str) -> anyhow::Result<SourceResult> {
        let gql = serde_json::json!({
            "query": r#"query GetArtistDetailsWithCuratorsWeb($pandoraId: String!) {
                entity(id: $pandoraId) {
                    name
                    topTracksWithCollaborations {
                        pandoraId
                        name
                        artistName
                        duration
                        shareableUrlPath
                        icon { artId }
                    }
                }
            }"#,
            "variables": { "pandoraId": id }
        });

        let auth = self.ensure_auth().await?;

        let resp = self
            .client
            .post("https://www.pandora.com/api/v1/pandora/graphql".to_string())
            .headers(Self::headers(&auth))
            .json(&gql)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("Pandora GraphQL error: {}", resp.status());
        }

        let data: Value = resp.json().await?;
        let entity = data.get("data").and_then(|d| d.get("entity"));
        let name = entity
            .and_then(|e| e.get("name").and_then(|v| v.as_str()))
            .unwrap_or("Artist");
        let top_tracks = entity
            .and_then(|e| e.get("topTracksWithCollaborations"))
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        let tracks: Vec<TrackData> = top_tracks
            .iter()
            .filter_map(Self::parse_annotation)
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: format!("{}'s Top Tracks", name),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "artist"}),
                tracks,
            },
        })
    }

    async fn resolve_podcast(&self, id: &str) -> anyhow::Result<SourceResult> {
        let body = serde_json::json!({
            "pandoraId": id
        });
        let data = self.api_post("/api/v1/podcast/metadata", body).await?;
        let details = data.get("details").and_then(|d| d.as_object()).cloned().unwrap_or_default();
        let annotations = details
            .get("annotations")
            .and_then(|a| a.as_object())
            .cloned()
            .unwrap_or_default();

        let tracks: Vec<TrackData> = annotations
            .values()
            .filter_map(Self::parse_annotation)
            .collect();

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }

        Ok(SourceResult::Playlist {
            data: PlaylistData {
                encoded: String::new(),
                info: PlaylistInfo {
                    name: "Podcast".into(),
                    selected_track: 0,
                },
                plugin_info: serde_json::json!({"type": "podcast"}),
                tracks,
            },
        })
    }
}

#[async_trait]
impl SourceProvider for PandoraSource {
    fn name(&self) -> &'static str {
        "pandora"
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["pdsearch"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        self.search_inner(query).await
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let re = Self::pattern();
        let caps = match re.captures(query) {
            Some(c) => c,
            None => return Ok(SourceResult::Empty),
        };
        let r#type = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let last_part = query.rsplit('/').next().unwrap_or("");

        if r#type.is_empty() || last_part.is_empty() {
            return Ok(SourceResult::Empty);
        }

        match r#type {
            "playlist" => self.resolve_playlist(last_part).await,
            "station" => self.resolve_station(last_part).await,
            "artist" => self.resolve_artist(last_part).await,
            "podcast" => self.resolve_podcast(last_part).await,
            _ => Ok(SourceResult::Empty),
        }
    }

    async fn get_track_url(&self, _track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: serde_json::Value::Null,
            new_track: None,
            additional_data: serde_json::Value::Object(serde_json::Map::new()),
            exception: Some("Pandora source does not provide stream URLs.".into()),
        })
    }
}
