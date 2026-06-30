#![allow(dead_code)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tracks::TrackData;

pub mod structs;
pub mod hls;
pub mod dash;
pub mod demuxers;
pub mod dsp;
pub mod processors;
pub mod opus;
pub mod resource;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerVoiceState {
    pub session_id: Option<String>,
    pub token: Option<String>,
    pub endpoint: Option<String>,
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerEventState {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
}

impl Default for PlayerEventState {
    fn default() -> Self {
        Self {
            time: 0,
            position: 0,
            connected: false,
            ping: -1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FiltersState {
    #[serde(default)]
    pub filters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FadingSection {
    pub duration: u64,
    pub curve: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FadingConfig {
    pub enabled: Option<bool>,
    pub track_start: Option<FadingSection>,
    pub track_end: Option<FadingSection>,
    pub track_stop: Option<FadingSection>,
    pub seek: Option<FadingSection>,
    pub pause: Option<FadingSection>,
    pub resume: Option<FadingSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateJson {
    pub guild_id: String,
    pub track: Option<TrackData>,
    pub volume: u32,
    pub fading: Option<FadingConfig>,
    pub loudness_normalizer: bool,
    pub paused: bool,
    pub filters: FiltersState,
    pub state: PlayerEventState,
    pub voice: PlayerVoiceState,
}

#[async_trait]
pub trait AudioResource: Send + Sync {
    async fn set_volume(&mut self, volume: u32) -> anyhow::Result<()>;
    async fn set_filters(&mut self, filters: FiltersState) -> anyhow::Result<()>;
    async fn destroy(&mut self) -> anyhow::Result<()>;
}

#[async_trait]
pub trait PlayerEngine: Send + Sync {
    async fn play(&mut self, track: TrackData) -> anyhow::Result<()>;
    async fn stop(&mut self) -> anyhow::Result<()>;
    async fn pause(&mut self, paused: bool) -> anyhow::Result<()>;
    async fn seek(&mut self, position_ms: u64) -> anyhow::Result<()>;
    fn snapshot(&self) -> PlayerStateJson;
}
