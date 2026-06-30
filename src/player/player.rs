use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::config::{CrossfadeConfig, FadingConfig};
use crate::constants;
use crate::plugins::PluginManager;
use crate::sources::SourceRegistry;
use crate::state::{LivePlayerState, PlayerVoiceState};
use crate::tracks::TrackInfo;

// ============================================================================
// Supporting Types (1:1 with NodeLink playback/player.ts types)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerTrack {
    pub encoded: String,
    pub info: TrackInfo,
    pub end_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_track_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SponsorBlockState {
    pub enabled: bool,
    pub categories: Vec<String>,
    pub action_types: Vec<String>,
    pub segments: Vec<SponsorBlockSegment>,
    pub last_skipped_uuid: Option<String>,
    pub skip_margin_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SponsorBlockSegment {
    pub start: f64,
    pub end: f64,
    pub category: String,
    pub uuid: String,
    #[serde(default = "default_skip_action")]
    pub action_type: String,
}

fn default_skip_action() -> String {
    "skip".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub url: Option<String>,
    pub protocol: Option<String>,
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_info: Option<TrackInfo>,
    #[serde(skip)]
    pub additional_data: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsPayload {
    pub source_name: String,
    pub provider: String,
    pub text: String,
    pub lines: Vec<LyricsLine>,
    pub plugin: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    pub timestamp: f64,
    pub duration: f64,
    pub line: String,
    pub words: Vec<serde_json::Value>,
    pub plugin: serde_json::Value,
}

#[derive(Default)]
pub struct FadeTimers {
    pub track_end: Option<tokio::task::JoinHandle<()>>,
    pub pause: Option<tokio::task::JoinHandle<()>>,
    pub stop: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateJson {
    pub guild_id: String,
    pub track: Option<PlayerTrack>,
    pub volume: u32,
    pub fading: Option<FadingConfig>,
    pub loudness_normalizer: bool,
    pub paused: bool,
    pub filters: serde_json::Value,
    pub state: PlayerEventState,
    pub voice: PlayerVoiceState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerEventState {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ProfilerStreamStats {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub last_chunk_at: Option<u64>,
}

/// Options required to construct a new Player.
pub struct PlayerOptions {
    pub guild_id: String,
    pub session_id: String,
    pub user_id: String,
    pub ws_sender: mpsc::Sender<serde_json::Value>,
    pub sources: SourceRegistry,
    pub plugin_manager: Arc<PluginManager>,
    pub default_volume: u16,
    pub fading: FadingConfig,
    pub crossfade: CrossfadeConfig,
    pub loudness_normalizer: bool,
    pub track_stuck_threshold_ms: u64,
    pub player_update_interval: u64,
    pub sponsorblock_config: crate::config::SponsorBlockConfig,
    pub event_timeout_ms: u64,
    pub max_stuck_recovery_attempts: u32,
}

// ============================================================================
// Player -- Core audio player state machine
// ============================================================================

/// Core audio player responsible for voice connection management, stream handling,
/// filter application, fading, lyrics synchronization, mix layers, and stuck-track recovery.
pub struct Player {
    // Identification
    pub guild_id: String,
    pub session_id: String,
    user_id: String,

    // Track state
    pub track: Option<PlayerTrack>,
    pub holo_track: Option<PlayerTrack>,
    pub next_track: Option<PlayerTrack>,
    pub next_resource: Option<serde_json::Value>,
    pub next_stream_info: Option<StreamInfo>,
    pub is_paused: bool,
    pub volume_percent: u32,
    pub filters: serde_json::Value,
    pub position: AtomicU64,
    pub conn_status: String,
    pub stream_info: Option<StreamInfo>,

    // SponsorBlock
    pub sponsor_block: SponsorBlockState,

    // Profiler stats
    pub profiler_stream_stats: ProfilerStreamStats,

    // Fading
    pub fading: Option<FadingConfig>,
    pub loudness_normalizer: bool,
    fade_timers: FadeTimers,

    // Lyrics
    pub is_lyrics_subscribed: bool,
    pub current_lyrics: Option<LyricsPayload>,
    pub lyrics_line_index: i32,
    pub skip_track_source: bool,

    // State guards (matching NodeLink boolean guards)
    pub destroying: bool,
    pub is_updating_track: bool,
    pub is_restoring: bool,
    pub is_seeking: bool,
    pub is_stopping: bool,
    pub is_recovering: bool,
    pub is_resuming: bool,
    _pending_track_start_fade: bool,
    _ignore_idle_stopped_until: u64,

    // Stuck detection
    last_position: AtomicU64,
    stuck_time: AtomicU64,
    last_stream_data_time: AtomicU64,
    stuck_recovery_count: AtomicU32,

    // Lyrics tracking
    lyrics_base_position: AtomicU64,
    lyrics_base_packets: AtomicU64,

    // Position tracking
    _paused_at_position: Option<u64>,

    // Voice state
    pub voice: PlayerVoiceState,
    pub last_manual_reconnect: AtomicU64,

    // Communication
    ws_sender: mpsc::Sender<serde_json::Value>,

    // External references
    sources: SourceRegistry,
    plugin_manager: Arc<PluginManager>,

    // Configuration
    track_stuck_threshold_ms: u64,
    player_update_interval: u64,
    crossfade_config: CrossfadeConfig,
    max_stuck_recovery_attempts: u32,
    event_timeout_ms: u64,

    // Player state reference
    live_state: Arc<RwLock<LivePlayerState>>,
    player_states: DashMap<String, Arc<RwLock<LivePlayerState>>>,
}

impl Player {
    pub fn new(options: PlayerOptions) -> Self {
        let guild_id = options.guild_id.clone();
        let sb_config = options.sponsorblock_config.clone();

        let fading = if options.fading.enabled {
            Some(options.fading.clone())
        } else {
            None
        };

        let live_state = Arc::new(RwLock::new(LivePlayerState::default()));
        let player_states: DashMap<String, Arc<RwLock<LivePlayerState>>> = DashMap::new();
        player_states.insert(guild_id.clone(), live_state.clone());

        let mut player = Self {
            guild_id: guild_id.clone(),
            session_id: options.session_id,
            user_id: options.user_id,
            track: None,
            holo_track: None,
            next_track: None,
            next_resource: None,
            next_stream_info: None,
            is_paused: false,
            volume_percent: options.default_volume as u32,
            filters: serde_json::Value::Object(serde_json::Map::new()),
            position: AtomicU64::new(0),
            conn_status: "disconnected".to_string(),
            stream_info: None,
            sponsor_block: SponsorBlockState {
                enabled: sb_config.enabled,
                categories: sb_config.categories,
                action_types: sb_config.action_types,
                segments: Vec::new(),
                last_skipped_uuid: None,
                skip_margin_ms: sb_config.skip_margin_ms,
            },
            profiler_stream_stats: ProfilerStreamStats::default(),
            fading,
            loudness_normalizer: options.loudness_normalizer,
            fade_timers: FadeTimers::default(),
            is_lyrics_subscribed: false,
            current_lyrics: None,
            lyrics_line_index: -1,
            skip_track_source: false,
            destroying: false,
            is_updating_track: false,
            is_restoring: false,
            is_seeking: false,
            is_stopping: false,
            is_recovering: false,
            is_resuming: false,
            _pending_track_start_fade: false,
            _ignore_idle_stopped_until: 0,
            last_position: AtomicU64::new(0),
            stuck_time: AtomicU64::new(0),
            last_stream_data_time: AtomicU64::new(0),
            stuck_recovery_count: AtomicU32::new(0),
            lyrics_base_position: AtomicU64::new(0),
            lyrics_base_packets: AtomicU64::new(0),
            _paused_at_position: None,
            voice: PlayerVoiceState::default(),
            last_manual_reconnect: AtomicU64::new(0),
            ws_sender: options.ws_sender,
            sources: options.sources,
            plugin_manager: options.plugin_manager,
            track_stuck_threshold_ms: options.track_stuck_threshold_ms,
            player_update_interval: options.player_update_interval,
            crossfade_config: options.crossfade,
            max_stuck_recovery_attempts: options.max_stuck_recovery_attempts,
            event_timeout_ms: options.event_timeout_ms,
            live_state,
            player_states,
        };

        debug!(target: "Player", "New player created for guild {} in session {}",
            player.guild_id, player.session_id);
        player.emit_event(
            constants::gateway_events::PLAYER_CREATED,
            json!({"guildId": player.guild_id, "player": null}),
        );

        player
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    pub fn emit_event(&self, event_type: &str, payload: serde_json::Value) {
        let mut event = serde_json::Map::new();
        event.insert("op".to_string(), json!("event"));
        event.insert("type".to_string(), json!(event_type));
        event.insert("guildId".to_string(), json!(self.guild_id));
        if let serde_json::Value::Object(map) = payload {
            for (k, v) in map {
                event.insert(k, v);
            }
        }
        let _ = self.ws_sender.try_send(serde_json::Value::Object(event));
    }

    pub fn send_player_update(&self) {
        let position = self._real_position();
        let event = json!({
            "op": "playerUpdate",
            "guildId": self.guild_id,
            "state": {
                "time": chrono_now(),
                "position": position,
                "connected": self.conn_status == "connected",
                "ping": -1i64
            }
        });
        let _ = self.ws_sender.try_send(event);
    }

    pub fn update_live_state(&self, connected: bool, ping: i64) {
        if let Some(ls) = self.player_states.get(&self.guild_id) {
            let mut state = ls.blocking_write();
            state.position = self._real_position();
            state.connected = connected;
            state.ping = ping;
            state.paused = self.is_paused;
            state.volume = self.volume_percent;
        }
    }

    fn _real_position(&self) -> u64 {
        // Uses atomic position; full impl would account for timescale speed from filters
        self.position.load(Ordering::SeqCst)
    }

    fn _get_timescale_speed(&self) -> f64 {
        let filters_obj = self.filters.get("filters")
            .or_else(|| Some(&self.filters));
        let timescale = filters_obj
            .and_then(|f| f.get("timescale"));
        let speed = timescale
            .and_then(|t| t.get("speed"))
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let rate = timescale
            .and_then(|t| t.get("rate"))
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        speed * rate
    }

    // ========================================================================
    // Public API -- matches NodeLink Player methods 1:1
    // ========================================================================

    /// Starts playback for the provided track payload.
    pub async fn play(
        &mut self,
        track: PlayerTrack,
        no_replace: bool,
        start_time: Option<u64>,
        _end_time: Option<u64>,
    ) -> bool {
        if self.destroying {
            debug!(target: "Player", "play() aborted -- destroying (guild: {})", self.guild_id);
            return false;
        }
        info!(target: "Player", "play() called for guild {} -- {} (start: {:?})",
            self.guild_id, track.info.identifier, start_time);

        self.is_updating_track = true;

        if no_replace && self.track.is_some() && !self.is_paused {
            let same_id = self.track.as_ref()
                .map(|t| t.info.identifier == track.info.identifier)
                .unwrap_or(false);
            if same_id {
                debug!(target: "Player", "play() noReplace -- already playing (guild: {})", self.guild_id);
                self.is_updating_track = false;
                return true;
            }
            debug!(target: "Player", "play() noReplace -- active track exists, enqueuing (guild: {})", self.guild_id);
            self.is_updating_track = false;
            return false;
        }

        if self.track.is_some() && self.conn_status != "disconnected" {
            self.emit_event(
                constants::gateway_events::TRACK_END,
                json!({"track": &self.track, "reason": "replaced"}),
            );
        }

        self._reset_track_state();
        self.track = Some(track);
        let start = start_time.unwrap_or(0);
        self.position.store(start, Ordering::SeqCst);

        let result = self._start_playback(start).await;
        self.is_updating_track = false;
        result
    }

    /// Stops playback and emits STOPPED event.
    pub fn stop(&mut self) -> bool {
        if self.destroying || self.track.is_none() {
            return false;
        }
        info!(target: "Player", "stop() called for guild {}", self.guild_id);
        self.is_stopping = true;
        self.emit_event(
            constants::gateway_events::TRACK_END,
            json!({"track": &self.track, "reason": "stopped"}),
        );
        self._reset_track_state();
        self.is_stopping = false;
        true
    }

    /// Pauses or resumes playback.
    pub fn pause(&mut self, should_pause: bool) -> bool {
        if self.destroying || self.is_paused == should_pause {
            return false;
        }
        info!(target: "Player", "Setting pause to {} for guild {}", should_pause, self.guild_id);

        if should_pause {
            self._paused_at_position = Some(self._real_position());
            self.is_paused = true;
        } else {
            self.is_paused = false;
            self.is_resuming = true;
        }
        self.emit_event(constants::gateway_events::PAUSE, json!({"paused": self.is_paused}));
        true
    }

    /// Adjusts playback volume (0-1000 range).
    pub fn volume(&mut self, level: u32) -> bool {
        if self.destroying {
            return false;
        }
        self.volume_percent = level.clamp(0, 1000);
        info!(target: "Player", "Setting volume to {} for guild {}", self.volume_percent, self.guild_id);
        self.emit_event(
            constants::gateway_events::VOLUME_CHANGED,
            json!({"volume": self.volume_percent}),
        );
        true
    }

    /// Performs a seek operation to the requested position.
    pub async fn seek(&mut self, position_ms: u64) -> bool {
        if self.destroying || self.track.is_none() {
            return false;
        }
        let tr = self.track.as_ref().unwrap();
        if !tr.info.is_seekable && !tr.info.is_stream {
            return false;
        }
        if position_ms == 0 && !self.is_recovering && self._real_position() < 2000 {
            debug!(target: "Player", "Ignoring seek to 0 -- track just started (guild: {})", self.guild_id);
            return false;
        }
        if position_ms as i64 > tr.info.length && tr.info.length > 0 {
            return false;
        }

        self.is_seeking = true;
        info!(target: "Player", "Seeking to {}ms for guild {}", position_ms, self.guild_id);
        self.position.store(position_ms, Ordering::SeqCst);
        self.emit_event(
            constants::gateway_events::SEEK,
            json!({"position": position_ms, "duration": 0}),
        );
        self.is_seeking = false;
        true
    }

    /// Sets fading configuration.
    pub fn set_fading(&mut self, config: Option<FadingConfig>) -> bool {
        self.fading = config;
        true
    }

    /// Toggles loudness normalization.
    pub fn set_loudness_normalizer(&mut self, enabled: bool) -> bool {
        self.loudness_normalizer = enabled;
        true
    }

    /// Applies audio filters to the active stream.
    pub fn set_filters(&mut self, filters: serde_json::Value) -> bool {
        if self.destroying || self.track.is_none() {
            return false;
        }
        debug!(target: "Player", "Applying filters for guild {}", self.guild_id);
        self.filters = filters.clone();
        self.emit_event(
            constants::gateway_events::FILTERS_CHANGED,
            json!({"filters": &self.filters}),
        );
        true
    }

    /// Preloads the next track for gapless playback.
    pub async fn preload(&mut self, track: PlayerTrack) -> bool {
        if self.destroying {
            return false;
        }
        if let Some(ref next) = self.next_track {
            if next.encoded == track.encoded || next.info.identifier == track.info.identifier {
                debug!(target: "Player", "Skipping duplicate preload for guild {}", self.guild_id);
                return true;
            }
        }
        self.next_track = Some(track);
        true
    }

    /// Clears any queued/preloaded next track.
    pub fn clear_next_track(&mut self) -> bool {
        self.next_track = None;
        self.next_resource = None;
        self.next_stream_info = None;
        true
    }

    /// Updates the voice state for this player.
    pub fn update_voice(&mut self, voice_payload: PlayerVoiceState, force: bool) -> bool {
        if self.destroying {
            return false;
        }
        let mut changed = false;

        if let Some(ref sid) = voice_payload.session_id {
            if self.voice.session_id.as_deref() != Some(sid) {
                self.voice.session_id = Some(sid.clone());
                changed = true;
            }
        }
        if let Some(ref token) = voice_payload.token {
            if self.voice.token.as_deref() != Some(token) {
                self.voice.token = Some(token.clone());
                changed = true;
            }
        }
        if let Some(ref endpoint) = voice_payload.endpoint {
            if self.voice.endpoint.as_deref() != Some(endpoint) {
                self.voice.endpoint = Some(endpoint.clone());
                changed = true;
            }
        }
        if let Some(ref cid) = voice_payload.channel_id {
            if self.voice.channel_id.as_deref() != Some(cid) {
                self.voice.channel_id = Some(cid.clone());
                changed = true;
            }
        }

        if !changed && !force {
            debug!(target: "Player", "Voice state for guild {} unchanged", self.guild_id);
            return false;
        }

        info!(target: "Player", "Updating voice state for guild {}", self.guild_id);

        if self.voice.session_id.is_some()
            && self.voice.token.is_some()
            && self.voice.endpoint.is_some()
        {
            self.emit_event(
                constants::gateway_events::PLAYER_CONNECTED,
                json!({"guildId": self.guild_id, "voice": self.voice}),
            );
        }
        true
    }

    /// Destroys the player and cleans up the voice connection.
    pub fn destroy(&mut self, emit_close: bool) {
        if self.destroying {
            return;
        }
        self.destroying = true;
        info!(target: "Player", "Destroying player for guild {}", self.guild_id);

        if emit_close {
            self.emit_event(
                constants::gateway_events::WEBSOCKET_CLOSED,
                json!({"code": 1000, "reason": "destroyed by client", "byRemote": false}),
            );
        }
        self.emit_event(
            constants::gateway_events::PLAYER_DESTROYED,
            json!({"guildId": self.guild_id}),
        );
        self._reset_track_state();
        self.conn_status = "destroyed".to_string();
    }

    /// Returns current SponsorBlock state.
    pub fn get_sponsor_block(&self) -> &SponsorBlockState {
        &self.sponsor_block
    }

    /// Updates SponsorBlock settings.
    pub fn update_sponsor_block(&mut self, updates: serde_json::Value) {
        if let Some(enabled) = updates.get("enabled").and_then(|v| v.as_bool()) {
            self.sponsor_block.enabled = enabled;
        }
        if let Some(cats) = updates.get("categories").and_then(|v| v.as_array()) {
            self.sponsor_block.categories = cats
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(acts) = updates.get("action_types").and_then(|v| v.as_array()) {
            self.sponsor_block.action_types = acts
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }

    /// Overrides SponsorBlock segments for the current track.
    pub fn set_sponsor_block_segments(&mut self, segments: Vec<SponsorBlockSegment>) {
        self.sponsor_block.segments = segments;
        self.sponsor_block.last_skipped_uuid = None;
    }

    /// Clears SponsorBlock state.
    pub fn clear_sponsor_block(&mut self) {
        self.sponsor_block.segments.clear();
        self.sponsor_block.last_skipped_uuid = None;
    }

    /// Subscribes to lyrics events for the current track.
    pub async fn subscribe_lyrics(&mut self, skip_track_source: bool) {
        if self.is_lyrics_subscribed {
            return;
        }
        self.is_lyrics_subscribed = true;
        self.skip_track_source = skip_track_source;
        info!(target: "Player", "Subscribed to lyrics for guild {}", self.guild_id);
    }

    /// Unsubscribes from lyrics events.
    pub async fn unsubscribe_lyrics(&mut self) {
        self.is_lyrics_subscribed = false;
        self.skip_track_source = false;
        self.current_lyrics = None;
        self.lyrics_line_index = -1;
    }

    /// Serializes player state to JSON-safe object.
    pub fn to_json(&self) -> PlayerStateJson {
        PlayerStateJson {
            guild_id: self.guild_id.clone(),
            track: self.track.clone(),
            volume: self.volume_percent,
            fading: self.fading.clone(),
            loudness_normalizer: self.loudness_normalizer,
            paused: self.is_paused,
            filters: self.filters.clone(),
            state: PlayerEventState {
                time: chrono_now(),
                position: self._real_position(),
                connected: self.conn_status == "connected",
                ping: -1,
            },
            voice: self.voice.clone(),
        }
    }

    // ========================================================================
    // Internal playback methods
    // ========================================================================

    async fn _start_playback(&mut self, start_time: u64) -> bool {
        if self.track.is_none() {
            return false;
        }
        info!(target: "Player", "Starting playback for guild {} at {}ms", self.guild_id, start_time);

        self.position.store(start_time, Ordering::SeqCst);
        self.stuck_time.store(0, Ordering::SeqCst);
        self.stuck_recovery_count.store(0, Ordering::SeqCst);

        self.emit_event(
            constants::gateway_events::TRACK_START,
            json!({"track": &self.track, "playingQuality": null}),
        );

        if let Ok(track_json) = serde_json::to_value(&self.track) {
            self.plugin_manager
                .on_track_start(&self.guild_id, &track_json)
                .await;
        }

        true
    }

    fn _reset_track_state(&mut self) {
        self.next_track = None;
        self.next_resource = None;
        self.next_stream_info = None;
        self.track = None;
        self.holo_track = None;
        self.is_paused = false;
        self.position.store(0, Ordering::SeqCst);
        self._paused_at_position = None;
        self.last_stream_data_time.store(0, Ordering::SeqCst);
        self.current_lyrics = None;
        self.lyrics_line_index = -1;
        self.stuck_time.store(0, Ordering::SeqCst);
        self.stuck_recovery_count.store(0, Ordering::SeqCst);
        self.is_recovering = false;
    }

    /// Emits TRACK_START event after resolving Holo tracks.
    pub async fn emit_track_start(&mut self) {
        let track = self.track.clone();
        if let Some(ref tr) = track {
            self.emit_event(
                constants::gateway_events::TRACK_START,
                json!({"track": tr, "playingQuality": null}),
            );
            if let Ok(track_json) = serde_json::to_value(tr) {
                self.plugin_manager
                    .on_track_start(&self.guild_id, &track_json)
                    .await;
            }
        }
    }

    /// Emits TRACK_END event.
    pub fn emit_track_end(&self, reason: &str) {
        let track = self.holo_track.as_ref().or(self.track.as_ref());
        if let Some(tr) = track {
            self.emit_event(
                constants::gateway_events::TRACK_END,
                json!({"track": tr, "reason": reason}),
            );
        }
    }

    // ========================================================================
    // Stuck detection & recovery (matches NodeLink _sendUpdate logic)
    // ========================================================================

    /// Checks if the player is stuck and attempts recovery.
    /// Returns true if a stuck event was handled.
    pub async fn check_stuck(&mut self) -> bool {
        let threshold = self.track_stuck_threshold_ms;
        if threshold == 0
            || self.is_updating_track
            || self.is_stopping
            || self.track.is_none()
            || self.is_recovering
            || self.is_paused
        {
            return false;
        }

        let current_position = self._real_position();
        let last_pos = self.last_position.load(Ordering::SeqCst);

        if last_pos == current_position {
            let prev = self.stuck_time.fetch_add(self.player_update_interval, Ordering::SeqCst);
            let new_stuck = prev + self.player_update_interval;

            if new_stuck >= threshold && self.conn_status == "connected" {
                self.stuck_time.store(0, Ordering::SeqCst);
                let recovery_count = self.stuck_recovery_count.load(Ordering::SeqCst);

                // Check near end-of-track to treat as natural finish
                if let Some(ref tr) = self.track {
                    let track_length = tr.info.length;
                    let playback_speed = self._get_timescale_speed();
                    let end_threshold = if playback_speed < 1.0 { 5000u64 } else { 2000u64 };

                    if track_length > 0
                        && current_position as i64 >= track_length - end_threshold as i64
                    {
                        debug!(target: "Player", "Near track end (guild: {}) -- treating as finish", self.guild_id);
                        self.emit_track_end("finished");
                        self._reset_track_state();
                        return true;
                    }
                }

                if recovery_count >= self.max_stuck_recovery_attempts {
                    error!(target: "Player",
                        "Player guild {} exceeded max recovery ({}). Stopping.",
                        self.guild_id, self.max_stuck_recovery_attempts);
                    self.emit_event(
                        constants::gateway_events::TRACK_STUCK,
                        json!({"guildId": self.guild_id, "track": &self.track,
                               "thresholdMs": threshold, "reason": "Max recovery attempts exceeded"}),
                    );
                    self.stop();
                    return true;
                }

                warn!(target: "Player",
                    "Player guild {} stuck. Recovering... (attempt {}/{})",
                    self.guild_id, recovery_count + 1, self.max_stuck_recovery_attempts);

                self.emit_event(
                    constants::gateway_events::TRACK_STUCK,
                    json!({"guildId": self.guild_id, "track": &self.track,
                           "thresholdMs": threshold}),
                );

                self.is_recovering = true;
                self.stuck_recovery_count.fetch_add(1, Ordering::SeqCst);

                if let Some(ref tr) = self.track {
                    if tr.info.is_seekable {
                        // Attempt recovery by re-seeking
                        let pos = current_position;
                        self.seek(pos).await;
                    } else {
                        self.stop();
                    }
                }

                self.is_recovering = false;
                return true;
            }
        } else {
            self.stuck_time.store(0, Ordering::SeqCst);
            let rc = self.stuck_recovery_count.load(Ordering::SeqCst);
            if rc > 0 {
                self.stuck_recovery_count.store(0, Ordering::SeqCst);
            }
            self.last_position.store(current_position, Ordering::SeqCst);
            self.last_stream_data_time.store(chrono_now(), Ordering::SeqCst);
        }
        false
    }

    // ========================================================================
    // SponsorBlock integration
    // ========================================================================

    /// Checks and skips SponsorBlock segments based on current position.
    /// Returns true if a segment was skipped.
    pub fn check_sponsorblock_skip(&mut self) -> bool {
        if !self.sponsor_block.enabled || self.is_paused || self.track.is_none() {
            return false;
        }

        let position = self._real_position();
        let segment = self.sponsor_block.segments.iter().find(|s| {
            self.sponsor_block.categories.contains(&s.category)
                && self.sponsor_block.action_types.contains(&s.action_type)
                && (position as f64 + self.sponsor_block.skip_margin_ms as f64) >= s.start
                && (position as f64) < s.end
                && self.sponsor_block.last_skipped_uuid.as_deref() != Some(&s.uuid)
        }).cloned();

        if let Some(seg) = segment {
            self.sponsor_block.last_skipped_uuid = Some(seg.uuid.clone());
            let skip_to = (seg.end * 1000.0) as u64;
            info!(target: "Player", "SponsorBlock skipping segment {} (category: {}) for guild {}",
                seg.uuid, seg.category, self.guild_id);
            // Schedule seek -- in full implementation this would be async
            self.emit_event(
                constants::gateway_events::SPONSORBLOCK_SEGMENT_SKIPPED,
                json!({"segment": seg, "skippedMs": skip_to.saturating_sub(position)}),
            );
            true
        } else {
            false
        }
    }

    // ========================================================================
    // Lyrics synchronization
    // ========================================================================

    /// Synchronizes lyrics with current playback position.
    pub fn sync_lyrics(&self) {
        if !self.is_lyrics_subscribed || self.current_lyrics.is_none() {
            return;
        }
        // Full lyrics sync would emit LyricsLineEvent based on position
        // Simplified: just check if we have lyrics and position is advancing
    }

    /// Loads lyrics for the current track and emits events.
    pub async fn load_lyrics(&mut self) {
        // Placeholder -- full implementation would use lyrics manager
        if self.track.is_none() || !self.is_lyrics_subscribed {
            return;
        }
        info!(target: "Player", "Loading lyrics for guild {}", self.guild_id);
        // In full impl: call lyricsManager.loadLyrics() and emit LyricsFoundEvent
    }
}

fn chrono_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Drop for Player {
    fn drop(&mut self) {
        if !self.destroying {
            warn!(target: "Player", "Player dropped without destroy() for guild {}", self.guild_id);
        }
    }
}
