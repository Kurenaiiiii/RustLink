use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::LyricsConfig;
use crate::lyrics::{
    self, deezer, genius, letrasmus, monochrome, musixmatch, yandex,
    LyricsData, SyncedLyricLine,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsResult {
    pub load_type: String,
    pub data: serde_json::Value,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsInfo {
    pub source: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub synced_lyrics: Vec<SyncedLyricLine>,
    pub plain_lyrics: Option<String>,
}

#[async_trait]
pub trait LyricsProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn fetch(&self, title: &str, artist: &str, album: Option<&str>) -> anyhow::Result<Option<LyricsInfo>>;
}

pub struct LyricsManager {
    client: reqwest::Client,
    config: LyricsConfig,
}

impl LyricsManager {
    pub fn new(config: LyricsConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    pub async fn load_lyrics(
        &self,
        title: &str,
        artist: &str,
        album: Option<&str>,
        source_name: Option<&str>,
        language: Option<&str>,
    ) -> anyhow::Result<LyricsResult> {
        let _ = language;

        // Try source-specific provider first
        if let Some(source) = source_name {
            if let Some(result) = self.try_source(source, title, artist, album).await? {
                return Ok(result);
            }
        }

        // Fallback: try all providers in order
        let providers: Vec<&str> = vec!["musixmatch", "genius", "deezer", "yandex", "monochrome", "letrasmus"];

        for provider in &providers {
            if self.is_enabled(provider) && Some(*provider) != source_name {
                if let Some(result) = self.try_source(provider, title, artist, album).await? {
                    return Ok(result);
                }
            }
        }

        // Try lrclib (always enabled, no config toggle needed)
        if let Some(lyrics) = self.fetch_lrclib(title, artist, album).await? {
            return Ok(LyricsResult {
                load_type: "lyrics".into(),
                data: serde_json::to_value(&lyrics)?,
                provider: Some("lrclib".into()),
            });
        }

        Ok(LyricsResult {
            load_type: "empty".into(),
            data: serde_json::Value::Null,
            provider: None,
        })
    }

    async fn try_source(
        &self,
        source: &str,
        title: &str,
        artist: &str,
        album: Option<&str>,
    ) -> anyhow::Result<Option<LyricsResult>> {
        let lyrics = match source {
            "musixmatch" => musixmatch::fetch_musixmatch(&self.client, title, artist).await?,
            "genius" => genius::fetch_genius(&self.client, title, artist).await?,
            "deezer" => deezer::fetch_deezer(&self.client, title, artist).await?,
            "yandex" => yandex::fetch_yandex(&self.client, title, artist).await?,
            "monochrome" => monochrome::fetch_monochrome(&self.client, title, artist).await?,
            "letrasmus" => letrasmus::fetch_letrasmus(&self.client, title, artist).await?,
            _ => None,
        };

        match lyrics {
            Some(data) => Ok(Some(LyricsResult {
                load_type: "lyrics".into(),
                data: serde_json::to_value(&data)?,
                provider: Some(source.to_string()),
            })),
            None => Ok(None),
        }
    }

    async fn fetch_lrclib(
        &self,
        title: &str,
        artist: &str,
        album: Option<&str>,
    ) -> anyhow::Result<Option<LyricsData>> {
        let mut url = format!(
            "https://lrclib.net/api/get?artist_name={}&track_name={}",
            lyrics::urlencoding(artist),
            lyrics::urlencoding(title)
        );
        if let Some(album) = album {
            url.push_str(&format!("&album_name={}", lyrics::urlencoding(album)));
        }

        match self.client.get(&url).send().await {
            Ok(resp) if resp.status() == reqwest::StatusCode::OK => {
                let data: serde_json::Value = resp.json().await?;
                let synced_raw = data["syncedLyrics"].as_str().unwrap_or("");

                Ok(Some(LyricsData {
                    source: "lrclib".into(),
                    title: data["trackName"].as_str().unwrap_or(title).to_string(),
                    artist: data["artistName"].as_str().unwrap_or(artist).to_string(),
                    album: data["albumName"].as_str().map(|s| s.to_string()),
                    synced_lyrics: lyrics::parse_lrc(synced_raw),
                    plain_lyrics: data["plainLyrics"].as_str().map(|s| s.to_string()),
                }))
            }
            _ => Ok(None),
        }
    }

    fn is_enabled(&self, source: &str) -> bool {
        match source {
            "musixmatch" => self.config.musixmatch.enabled,
            "genius" => self.config.genius.enabled,
            "deezer" => self.config.deezer.enabled,
            "yandex" => self.config.yandexmusic.enabled,
            "monochrome" => {
                // monochrome doesn't have a dedicated config toggle; always enabled
                true
            }
            "letrasmus" => self.config.letrasmus.enabled,
            _ => true,
        }
    }
}
