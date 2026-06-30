use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSMediaPlaylist {
    pub is_master: bool,
    pub is_live: bool,
    pub media_sequence: i64,
    pub target_duration: f64,
    pub segments: Vec<HLSSegment>,
    pub variants: Vec<HLSVariant>,
    pub audio_groups: HashMap<String, Vec<HLSAudioRendition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSVariant {
    pub bandwidth: u32,
    pub codecs: Option<String>,
    pub url: String,
    pub audio: Option<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSSegment {
    pub url: String,
    pub duration: f64,
    pub sequence: i64,
    pub discontinuity: bool,
    pub key: Option<HLSKey>,
    pub map: Option<HLSMap>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSKey {
    pub method: String,
    pub uri: Option<String>,
    pub iv: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSMap {
    pub uri: String,
    pub byterange: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSAudioRendition {
    pub default: Option<String>,
    pub autoselect: Option<String>,
    pub uri: Option<String>,
    pub name: Option<String>,
}

pub type HLSPlaylist = HLSMediaPlaylist;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HLSFetchStrategy {
    Segmented,
    Streaming,
    Auto,
}

#[derive(Default)]
pub struct HLSHandlerOptions {
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub local_address: Option<String>,
    pub proxy: Option<String>,
    pub on_resolve_url: Option<Box<dyn Fn(String) -> Option<String> + Send + Sync>>,
    pub strategy: Option<HLSFetchStrategy>,
    pub start_time: Option<f64>,
    pub high_water_mark: Option<usize>,
}