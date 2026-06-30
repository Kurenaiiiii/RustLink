use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::tracks::{Chapter, TrackData, TrackInfo};

pub mod amazonmusic;
pub mod applemusic;
pub mod bandcamp;
pub mod bilibili;
pub mod deezer;
pub mod eternalbox;
pub mod gaana;
pub mod googledrive;
pub mod jiosaavn;
pub mod pandora;
pub mod instagram;
pub mod soundcloud;
pub mod twitch;
pub mod youtube;
pub mod youtube_clients;
pub mod youtube_cipher;
pub mod youtube_common;
pub mod youtube_oauth;
pub mod youtube_potoken;
pub mod youtube_sabr;
pub mod reddit;
pub mod spotify;
pub mod stub;
pub mod telegram;
pub mod twitter;
pub mod vkmusic;
pub mod yandexmusic;
pub mod spotify_canvas;
pub mod youtube_livechat;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "loadType", rename_all = "camelCase")]
pub enum SourceResult {
    Track(TrackData),
    Playlist { data: PlaylistData },
    Search { data: Vec<TrackData> },
    Empty,
    Error(String),
}

impl SourceResult {
    pub fn empty() -> Self {
        SourceResult::Empty
    }

    pub fn error(msg: String) -> Self {
        SourceResult::Error(msg)
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistData {
    pub encoded: String,
    pub info: PlaylistInfo,
    pub plugin_info: serde_json::Value,
    pub tracks: Vec<TrackData>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInfo {
    pub name: String,
    pub selected_track: i32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TrackUrlResult {
    pub url: Option<String>,
    pub protocol: Option<String>,
    pub format: serde_json::Value,
    pub new_track: Option<TrackData>,
    pub additional_data: serde_json::Value,
    pub exception: Option<String>,
}

#[async_trait]
pub trait SourceProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    fn search_terms(&self) -> &'static [&'static str] {
        &[]
    }
    async fn search(&self, query: &str, search_type: Option<&str>) -> anyhow::Result<SourceResult>;
    async fn resolve(&self, query: &str, kind: Option<&str>) -> anyhow::Result<SourceResult>;
    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult>;
    async fn get_chapters(&self, _track: &TrackInfo) -> anyhow::Result<Vec<Chapter>> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
pub struct SourceRegistry {
    providers: Arc<RwLock<Vec<Box<dyn SourceProvider>>>>,
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn search(&self, source: &str, query: &str) -> anyhow::Result<SourceResult> {
        let providers = self.providers.read().await;
        for provider in providers.iter() {
            if provider.name() == source || provider.aliases().contains(&source) {
                return provider.search(query, None).await;
            }
        }
        Ok(SourceResult::Empty)
    }

    pub async fn search_with_default(
        &self,
        default_source: &str,
        query: &str,
    ) -> anyhow::Result<SourceResult> {
        // If query starts with "ytsearch:" or similar
        if let Some((source, query_rest)) = query.split_once(':') {
            let providers = self.providers.read().await;
            for provider in providers.iter() {
                if provider.search_terms().contains(&source) {
                    return provider.search(query_rest, Some(source)).await;
                }
            }
        }
        // Fallback to default search
        self.search(default_source, query).await
    }

    pub async fn register<P: SourceProvider + 'static>(&self, provider: P) {
        self.providers.write().await.push(Box::new(provider));
    }

    pub async fn resolve(&self, query: &str) -> anyhow::Result<SourceResult> {
        let providers = self.providers.read().await;
        for provider in providers.iter() {
            if let Ok(res) = provider.resolve(query, None).await {
                match res {
                    SourceResult::Empty => continue,
                    _ => return Ok(res),
                }
            }
        }
        Ok(SourceResult::Empty)
    }

    pub async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let providers = self.providers.read().await;
        for provider in providers.iter() {
            if provider.name() == track.source_name {
                return provider.get_track_url(track).await;
            }
        }
        Err(anyhow::anyhow!(
            "No provider found for track source: {}",
            track.source_name
        ))
    }

    pub async fn get_chapters(&self, track: &TrackInfo) -> anyhow::Result<Vec<Chapter>> {
        let providers = self.providers.read().await;
        for provider in providers.iter() {
            if provider.name() == track.source_name {
                return provider.get_chapters(track).await;
            }
        }
        Ok(Vec::new())
    }

    pub async fn source_names(&self) -> Vec<String> {
        let providers = self.providers.read().await;
        providers.iter().map(|p| p.name().to_string()).collect()
    }
}
