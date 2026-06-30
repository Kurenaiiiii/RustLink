//! Typings — comprehensive type definitions and serialization helpers
//! for all modules, ensuring consistent JSON interchange format.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- Load Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadTracksResponse {
    pub load_type: LoadType,
    pub data: LoadData,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum LoadType {
    Track,
    Playlist,
    Search,
    Empty,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum LoadData {
    Track(TrackData),
    Playlist(PlaylistData),
    Search(SearchData),
    Empty(EmptyData),
    Error(ErrorData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackData {
    pub encoded: Option<String>,
    pub info: TrackInfo,
    #[serde(default)]
    pub plugin_info: Value,
    #[serde(default)]
    pub user_data: Value,
    #[serde(default)]
    pub details: Vec<Option<String>>,
    #[serde(default)]
    pub message_flags: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub identifier: String,
    pub is_seekable: bool,
    pub author: String,
    pub length: i64,
    pub is_stream: bool,
    pub position: i64,
    pub title: String,
    pub uri: Option<String>,
    pub artwork_url: Option<String>,
    pub isrc: Option<String>,
    pub source_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapters: Option<Vec<crate::tracks::Chapter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistData {
    pub encoded: String,
    pub info: PlaylistInfo,
    #[serde(default)]
    pub plugin_info: Value,
    pub tracks: Vec<TrackData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInfo {
    pub name: String,
    pub selected_track: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchData {
    pub tracks: Vec<TrackData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyData {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorData {
    pub message: String,
    pub severity: String,
}

// --- Player Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub guild_id: String,
    pub track: Option<TrackData>,
    pub volume: u32,
    pub paused: bool,
    pub state: PlayerEventState,
    pub voice: PlayerVoiceState,
    pub filters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerEventState {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerVoiceState {
    pub session_id: Option<String>,
    pub token: Option<String>,
    pub endpoint: Option<String>,
    pub channel_id: Option<String>,
}

// --- Session Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub resuming: bool,
    pub timeout: u64,
    pub players: Vec<PlayerState>,
}

// --- WebSocket Event Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsEvent {
    pub op: WsOpCode,
    pub guild_id: Option<String>,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WsOpCode {
    #[serde(rename = "event")]
    Event,
    #[serde(rename = "playerUpdate")]
    PlayerUpdate,
    #[serde(rename = "stats")]
    Stats,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "error")]
    Error,
}

// --- Filter Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterPayload {
    pub volume: Option<f32>,
    pub equalizer: Option<Vec<EqBand>>,
    pub karaoke: Option<KaraokeSettings>,
    pub timescale: Option<TimescaleSettings>,
    pub tremolo: Option<TremoloSettings>,
    pub vibrato: Option<VibratoSettings>,
    pub rotation: Option<RotationSettings>,
    pub distortion: Option<DistortionSettings>,
    pub channel_mix: Option<ChannelMixSettings>,
    pub low_pass: Option<LowPassSettings>,
    pub high_pass: Option<HighPassSettings>,
    pub band_pass: Option<BandPassSettings>,
    pub echo: Option<EchoSettings>,
    pub reverb: Option<ReverbSettings>,
    pub flanger: Option<FlangerSettings>,
    pub phonograph: Option<PhonographSettings>,
    pub chorus: Option<ChorusSettings>,
    pub compressor: Option<CompressorSettings>,
    pub phaser: Option<PhaserSettings>,
    pub spatial: Option<SpatialSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqBand {
    pub band: i32,
    pub gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KaraokeSettings {
    pub level: f32,
    pub mono_level: f32,
    pub filter_band: f32,
    pub filter_width: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimescaleSettings {
    pub speed: f32,
    pub pitch: f32,
    pub rate: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TremoloSettings {
    pub frequency: f32,
    pub depth: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VibratoSettings {
    pub frequency: f32,
    pub depth: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationSettings {
    pub rotation_hz: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistortionSettings {
    pub sin_offset: f32,
    pub sin_scale: f32,
    pub cos_offset: f32,
    pub cos_scale: f32,
    pub tan_offset: f32,
    pub tan_scale: f32,
    pub offset: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMixSettings {
    pub left_to_left: f32,
    pub left_to_right: f32,
    pub right_to_left: f32,
    pub right_to_right: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LowPassSettings {
    pub smoothing: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighPassSettings {
    pub smoothing: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BandPassSettings {
    pub frequency: f32,
    pub bandwidth: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EchoSettings {
    pub delay: f32,
    pub decay: f32,
    pub max_delay: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverbSettings {
    pub room_size: f32,
    pub damping: f32,
    pub wet_level: f32,
    pub dry_level: f32,
    pub delay: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlangerSettings {
    pub delay: f32,
    pub depth: f32,
    pub rate: f32,
    pub feedback: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhonographSettings {
    pub crackle_volume: f32,
    pub pop_volume: f32,
    pub hum_volume: f32,
    pub low_pass_smoothing: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChorusSettings {
    pub delay: f32,
    pub depth: f32,
    pub rate: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressorSettings {
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub makeup_gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaserSettings {
    pub rate: f32,
    pub depth: f32,
    pub feedback_q: f32,
    pub center_freq: f32,
    pub stages: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpatialSettings {
    pub position: [f32; 3],
    pub rotation: f32,
    pub intensity: f32,
    pub algorithm: String,
}

// --- Stats Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsPayload {
    pub players: usize,
    pub playing_players: usize,
    pub uptime: u64,
    pub memory: MemoryStats,
    pub cpu: CpuStats,
    pub frame_stats: FrameStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    pub free: u64,
    pub used: u64,
    pub allocated: u64,
    pub reservable: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    pub cores: u32,
    pub system_load: f64,
    pub process_load: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameStats {
    pub sent: u64,
    pub nulled: u64,
    pub deficit: u64,
}

// --- Route Planner Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutePlannerStatus {
    pub class: String,
    pub details: RoutePlannerDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutePlannerDetails {
    pub ip_block: IpBlockInfo,
    pub failing_addresses: Vec<FailingAddress>,
    pub blocked_addresses: Vec<BlockedAddress>,
    pub rotating: bool,
    pub current_address_index: usize,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpBlockInfo {
    #[serde(rename = "type")]
    pub block_type: String,
    pub size: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailingAddress {
    pub address: String,
    pub failing_timestamp: u64,
    pub failing_time: String,
    pub unavailable_since: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedAddress {
    pub address: String,
    pub blocked_timestamp: u64,
    pub blocked: bool,
}

// --- Lyrics Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsResponse {
    pub lyrics: bool,
    pub source: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub synced_lyrics: Vec<SyncedLyricLine>,
    pub plain_lyrics: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedLyricLine {
    pub time: f64,
    pub text: String,
}
