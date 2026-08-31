use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

pub struct TwitchSource {
    client: Client,
    _client_id: Option<String>,
}

impl TwitchSource {
    pub fn new(client_id: Option<String>) -> Self {
        Self {
            client: Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .build()
                .unwrap(),
            _client_id: client_id,
        }
    }

    async fn gql_query(&self, query: &str, variables: Value) -> anyhow::Result<Value> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Client-Id",
            reqwest::header::HeaderValue::from_static("kimne78kx3ncx6brgo4mv6wki5h1ko"),
        );
        let body = json!({
            "query": query,
            "variables": variables,
        });
        let resp = self
            .client
            .post("https://gql.twitch.tv/gql")
            .headers(headers)
            .json(&body)
            .send()
            .await?;
        Ok(resp.error_for_status()?.json().await?)
    }
}

#[async_trait]
impl SourceProvider for TwitchSource {
    fn name(&self) -> &'static str {
        "twitch"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["tw", "twitch"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let gql = r#"
            query SearchChannels($query: String!) {
                searchFor(query: $query, options: {}) {
                    channels {
                        id
                        login
                        displayName
                        description
                        profileImageURL
                        stream {
                            title
                            type
                            viewersCount
                            game {
                                name
                            }
                        }
                    }
                }
            }
        "#;

        match self.gql_query(gql, json!({"query": query})).await {
            Ok(data) => {
                let channels = data
                    .pointer("/data/searchFor/channels")
                    .and_then(|c| c.as_array())
                    .map(|a| a.to_vec())
                    .unwrap_or_default();

                let mut tracks: Vec<TrackData> = Vec::new();
                for ch in &channels {
                    let login = ch["login"].as_str().unwrap_or("");
                    let display = ch["displayName"].as_str().unwrap_or(login);
                    let avatar = ch["profileImageURL"].as_str();
                    let stream_title = ch.pointer("/stream/title").and_then(|t| t.as_str());

                    let mut track = TrackData {
                        encoded: None,
                        info: TrackInfo {
                            identifier: login.to_string(),
                            is_seekable: false,
                            author: display.to_string(),
                            length: 0,
                            is_stream: true,
                            position: 0,
                            title: stream_title.unwrap_or(display).to_string(),
                            uri: Some(format!("https://twitch.tv/{login}")),
                            artwork_url: avatar.map(|s| s.to_string()),
                            isrc: None,
                            source_name: "twitch".to_string(),
                            chapters: None,
                        },
                        plugin_info: json!({}),
                        user_data: json!({}),
                        details: Vec::new(),
                        message_flags: 0,
                    };
                    track.encoded = Some(encode_track(&track));
                    tracks.push(track);
                }

                if tracks.is_empty() {
                    Ok(SourceResult::Empty)
                } else {
                    Ok(SourceResult::Search { data: tracks })
                }
            }
            Err(e) => {
                warn!(target: "Twitch", "Search error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let trimmed = query.trim();
        let channel = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let url = url::Url::parse(trimmed).ok();
            let host = url.as_ref().and_then(|u| u.host_str()).unwrap_or("");
            if host.contains("twitch.tv") || host.contains("twitch.com") {
                url.as_ref()
                    .and_then(|u| u.path_segments())
                    .and_then(|s| s.filter(|s| !s.is_empty()).next())
                    .map(|s| s.to_string())
            } else {
                None
            }
        } else if trimmed.starts_with("twitch:") {
            Some(trimmed["twitch:".len()..].to_string())
        } else {
            return self.search(trimmed, None).await;
        };

        let channel = match channel {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(SourceResult::Empty),
        };

        info!(target: "Twitch", "Resolving channel: {channel}");

        let gql = r#"
            query ChannelLookup($login: String!) {
                user(login: $login) {
                    id
                    login
                    displayName
                    description
                    profileImageURL
                    stream {
                        id
                        title
                        type
                        viewersCount
                        game {
                            name
                        }
                        previewImageURL
                    }
                }
            }
        "#;

        match self.gql_query(gql, json!({"login": channel})).await {
            Ok(data) => {
                let user = data.pointer("/data/user");
                let user = match user {
                    Some(u) if !u.is_null() => u,
                    _ => return Ok(SourceResult::Empty),
                };

                let login = user["login"].as_str().unwrap_or(&channel);
                let display = user["displayName"].as_str().unwrap_or(login);
                let avatar = user["profileImageURL"].as_str();
                let is_live = user.get("stream").and_then(|s| s.as_object()).is_some();
                let stream_title = user.pointer("/stream/title").and_then(|t| t.as_str());

                let title = if is_live {
                    stream_title.unwrap_or(display)
                } else {
                    display
                };

                let mut track = TrackData {
                    encoded: None,
                    info: TrackInfo {
                        identifier: login.to_string(),
                        is_seekable: false,
                        author: display.to_string(),
                        length: 0,
                        is_stream: true,
                        position: 0,
                        title: title.to_string(),
                        uri: Some(format!("https://twitch.tv/{login}")),
                        artwork_url: avatar.map(|s| s.to_string()),
                        isrc: None,
                        source_name: "twitch".to_string(),
                        chapters: None,
                    },
                    plugin_info: json!({"live": is_live}),
                    user_data: json!({}),
                    details: Vec::new(),
                    message_flags: 0,
                };
                track.encoded = Some(encode_track(&track));

                Ok(SourceResult::Track(track))
            }
            Err(e) => {
                warn!(target: "Twitch", "Resolve error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn get_track_url(&self, _track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        // Twitch stream URLs require OAuth token + client_id for HLS access
        // For now, return an error with instructions
        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: json!({}),
            new_track: None,
            additional_data: json!({}),
            exception: Some(
                "Twitch stream URLs require OAuth authentication. \
                 Set up a Twitch application and configure client_id/secret in the config."
                    .into(),
            ),
        })
    }
}
