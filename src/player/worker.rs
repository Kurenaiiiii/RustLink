use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use dashmap::DashMap;
use serde_json::json;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Instant;
use tracing::{error, info, warn};

use crate::config::{CrossfadeConfig, FadingConfig};
use crate::lyrics::fetch_lyrics;
use crate::plugins::PluginManager;
use crate::player::audio_pipeline::{self, AudioPipeline, ResampleQuality};
use crate::player::fading::{TapeAction, TapeFade, VolumeFade};
use crate::player::loudness::LoudnessNormalizer;
use crate::player::filter_chain::FilterChain;
use crate::player::mixer::AudioMixer;
use crate::player::voice::{VoiceConnection, VoiceSession};
use crate::sources::SourceRegistry;
use crate::sponsorblock::fetch_segments;
use crate::state::LivePlayerState;
use crate::tracks::{decode_track, TrackInfo};

pub struct QueueState {
    pub queue: Vec<String>,
    pub current_encoded: Option<String>,
    pub repeat: RepeatMode,
}

impl QueueState {
    pub fn new() -> Self {
        Self {
            queue: Vec::new(),
            current_encoded: None,
            repeat: RepeatMode::None,
        }
    }

    pub fn next_track(&mut self) -> Option<String> {
        match self.repeat {
            RepeatMode::One => self.current_encoded.clone(),
            RepeatMode::All => {
                if self.queue.is_empty() {
                    self.current_encoded.clone()
                } else {
                    let next = self.queue.remove(0);
                    self.queue.push(next.clone());
                    Some(next)
                }
            }
            RepeatMode::None => {
                if !self.queue.is_empty() {
                    Some(self.queue.remove(0))
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepeatMode {
    None,
    One,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerStatus {
    Idle,
    Playing,
    Paused,
    Stopped,
    Destroyed,
}

impl Default for PlayerStatus {
    fn default() -> Self {
        Self::Idle
    }
}

pub enum WorkerCommand {
    VoiceUpdate {
        session_id: String,
        user_id: String,
        token: String,
        endpoint: String,
    },
    Play {
        encoded_track: String,
        no_replace: bool,
    },
    Stop,
    Pause(bool),
    Volume(u16),
    Seek(u64),
    Filters(serde_json::Value),
    Destroy,
    NextTrack,
    Preload {
        encoded_track: String,
    },
    Repeat(RepeatMode),
    Shuffle,
    MixerAddLayer { name: String, volume: f32, pan: f32 },
    MixerRemoveLayer { layer_id: String },
    MixerUpdateLayer {
        layer_id: String,
        name: Option<String>,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    },
    MixerSetUrl { layer_id: String, url: Option<String> },
    MixerList,
    Subscribe { topic: String },
    Unsubscribe { topic: String },
}

pub struct PlayerWorker {
    pub guild_id: String,
    pub rx: mpsc::Receiver<WorkerCommand>,
    pub ws_sender: Option<mpsc::Sender<serde_json::Value>>,
    pub sources: SourceRegistry,
    status: PlayerStatus,
    is_playing: bool,
    paused: Arc<AtomicBool>,
    position: Arc<AtomicU64>,
    current_track: Option<TrackInfo>,
    current_track_encoded: Option<String>,
    next_track_encoded: Option<String>,
    queue_state: Arc<tokio::sync::Mutex<QueueState>>,
    playback_task: Option<tokio::task::JoinHandle<()>>,
    playback_cancelled: Arc<AtomicBool>,
    track_ended_notify: Arc<tokio::sync::Notify>,
    current_filters: Arc<RwLock<FilterChain>>,
    live_state: Arc<RwLock<LivePlayerState>>,
    player_states: DashMap<String, Arc<RwLock<LivePlayerState>>>,
    sponsorblock: DashMap<String, Vec<String>>,
    fade_config: FadingConfig,
    track_stuck_threshold_ms: u64,
    resample_quality: String,
    crossfade_config: CrossfadeConfig,
    loudness_normalizer: bool,
    lookahead_ms: u64,
    gate_threshold_lufs: f64,
    mixer: Arc<tokio::sync::Mutex<AudioMixer>>,
    subscriptions: Arc<tokio::sync::Mutex<HashSet<String>>>,
    plugin_manager: Arc<PluginManager>,
    sabr_session_state: Arc<std::sync::Mutex<Option<crate::sources::youtube_sabr::PreviousSessionState>>>,

    // State guards (match NodeLink's boolean guards)
    destroying: bool,
    is_updating_track: bool,
    is_seeking: bool,
    is_stopping: bool,
    is_recovering: bool,

    // Stuck track detection
    last_position: u64,
    stuck_time_ms: u64,
    stuck_recovery_count: u32,
    last_stream_data_time: tokio::time::Instant,
    max_stuck_recovery_attempts: u32,

    // Event queue for paused sessions
    event_queue: Vec<serde_json::Value>,
}

impl PlayerWorker {
    pub fn new(
        guild_id: String,
        rx: mpsc::Receiver<WorkerCommand>,
        ws_sender: Option<mpsc::Sender<serde_json::Value>>,
        sources: SourceRegistry,
        player_states: DashMap<String, Arc<RwLock<LivePlayerState>>>,
        sponsorblock: DashMap<String, Vec<String>>,
        fade_config: FadingConfig,
        track_stuck_threshold_ms: u64,
        resample_quality: String,
        crossfade_config: CrossfadeConfig,
        loudness_normalizer: bool,
        lookahead_ms: u64,
        gate_threshold_lufs: f64,
        plugin_manager: Arc<PluginManager>,
    ) -> Self {
        let live_state = Arc::new(RwLock::new(LivePlayerState::default()));
        player_states.insert(guild_id.clone(), live_state.clone());
        let now = tokio::time::Instant::now();
        Self {
            guild_id,
            rx,
            ws_sender,
            sources,
            status: PlayerStatus::Idle,
            is_playing: false,
            paused: Arc::new(AtomicBool::new(false)),
            position: Arc::new(AtomicU64::new(0)),
            current_track: None,
            current_track_encoded: None,
            next_track_encoded: None,
            queue_state: Arc::new(tokio::sync::Mutex::new(QueueState::new())),
            playback_task: None,
            playback_cancelled: Arc::new(AtomicBool::new(false)),
            track_ended_notify: Arc::new(tokio::sync::Notify::new()),
            current_filters: Arc::new(RwLock::new(FilterChain::default())),
            live_state,
            player_states,
            sponsorblock,
            fade_config,
            track_stuck_threshold_ms,
            resample_quality,
            crossfade_config,
            loudness_normalizer,
            lookahead_ms,
            gate_threshold_lufs,
            mixer: Arc::new(tokio::sync::Mutex::new(AudioMixer::new())),
            subscriptions: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            plugin_manager,
            sabr_session_state: Arc::new(std::sync::Mutex::new(None)),
            destroying: false,
            is_updating_track: false,
            is_seeking: false,
            is_stopping: false,
            is_recovering: false,
            last_position: 0,
            stuck_time_ms: 0,
            stuck_recovery_count: 0,
            last_stream_data_time: now,
            max_stuck_recovery_attempts: 3,
            event_queue: Vec::new(),
        }
    }

    fn send_ws(ws: &Option<mpsc::Sender<serde_json::Value>>, payload: serde_json::Value) {
        if let Some(ref sender) = ws {
            let _ = sender.try_send(payload);
        }
    }

    fn emit_event(&self, payload: serde_json::Value) {
        Self::send_ws(&self.ws_sender, payload);
    }

    fn next_queue_track(&self) -> Option<String> {
        None // redirects to queue_state in run()
    }

    fn emit_event_or_queue(&mut self, payload: serde_json::Value) {
        if self.paused.load(Ordering::Relaxed) {
            self.event_queue.push(payload);
        } else {
            self.emit_event(payload);
        }
    }

    fn flush_event_queue(&mut self) {
        let events = std::mem::take(&mut self.event_queue);
        for event in events {
            self.emit_event(event);
        }
    }

    async fn check_stuck(&mut self, session: Option<&mut VoiceSession>) {
        if self.status != PlayerStatus::Playing || self.is_recovering || self.track_stuck_threshold_ms == 0 {
            return;
        }
        let current_pos = self.position.load(Ordering::Relaxed);
        if current_pos == self.last_position {
            self.stuck_time_ms += 1000;
            if self.stuck_time_ms >= self.track_stuck_threshold_ms {
                // Track is stuck — attempt recovery
                self.is_recovering = true;
                self.stuck_recovery_count += 1;
                info!(target: "Worker", "Track stuck detected (attempt {}/{}) (guild: {})",
                    self.stuck_recovery_count, self.max_stuck_recovery_attempts, self.guild_id);

                let track_clone = self.current_track.clone();
                if let Some(ref track) = track_clone {
                    self.emit_event_or_queue(json!({
                        "op": "event",
                        "type": "TrackStuckEvent",
                        "guildId": self.guild_id,
                        "track": track,
                        "thresholdMs": self.track_stuck_threshold_ms
                    }));
                    self.plugin_manager.on_track_stuck(&self.guild_id,
                        &serde_json::to_value(track).unwrap_or_default(),
                        self.track_stuck_threshold_ms).await;
                }

                if self.stuck_recovery_count >= self.max_stuck_recovery_attempts {
                    // Max attempts reached — stop
                    let track_clone = self.current_track.clone();
                    if let Some(ref track) = track_clone {
                        self.emit_event_or_queue(json!({
                            "op": "event",
                            "type": "TrackEndEvent",
                            "guildId": self.guild_id,
                            "track": track,
                            "reason": "stopped"
                        }));
                    }
                    self.abort_current_playback();
                    self.is_playing = false;
                    self.status = PlayerStatus::Stopped;
                    self.is_recovering = false;
                    return;
                }

                // Attempt recovery by seeking to current position
                let encoded = self.current_track_encoded.clone().unwrap_or_default();
                self.abort_current_playback();
                if let Some(s) = session {
                    self.start_track(&encoded, s).await;
                }
                self.stuck_time_ms = 0;
                self.is_recovering = false;
            }
        } else {
            self.stuck_time_ms = 0;
            self.last_position = current_pos;
        }
    }

    fn abort_current_playback(&mut self) {
        self.playback_cancelled.store(true, Ordering::Relaxed);
        if let Some(task) = self.playback_task.take() {
            task.abort();
        }
    }

    #[allow(unused_assignments)]
    async fn start_track(
        &mut self,
        encoded_track: &str,
        voice_session: &mut VoiceSession,
    ) -> bool {
        let track = match decode_track(encoded_track) {
            Ok(t) => t,
            Err(e) => {
                error!(target: "Worker", "Failed to decode track: {} (guild: {})", e, self.guild_id);
                self.emit_event(json!({
                    "op": "event",
                    "type": "TrackExceptionEvent",
                    "guildId": self.guild_id,
                    "exception": {
                        "message": format!("Failed to decode track: {e}"),
                        "severity": "fault",
                        "cause": "Track Decode Error"
                    }
                }));
                return false;
            }
        };

        let track_info = track.info.clone();
        self.current_track = Some(track_info.clone());
        self.current_track_encoded = Some(encoded_track.to_string());

        let (url, protocol, sabr_additional) = match self.sources.get_track_url(&track.info).await {
            Ok(result) => {
                let prot = result.protocol.clone().unwrap_or_default();
                let extra = if prot == "sabr" {
                    result.additional_data.get("sabr").cloned()
                } else {
                    None
                };
                match result.url {
                    Some(u) => (u, prot, extra),
                    None => {
                        error!(target: "Worker", "Source returned no URL (guild: {})", self.guild_id);
                        self.emit_event(json!({
                            "op": "event",
                            "type": "TrackEndEvent",
                            "guildId": self.guild_id,
                            "track": &track_info,
                            "reason": "loadFailed"
                        }));
                        self.emit_event(json!({
                            "op": "event",
                            "type": "TrackExceptionEvent",
                            "guildId": self.guild_id,
                            "exception": {
                                "message": "Source returned no URL",
                                "severity": "common",
                                "cause": "No URL"
                            }
                        }));
                        return false;
                    }
                }
            },
            Err(e) => {
                error!(target: "Worker", "Failed to resolve track URL: {} (guild: {})", e, self.guild_id);
                self.emit_event(json!({
                    "op": "event",
                    "type": "TrackEndEvent",
                    "guildId": self.guild_id,
                    "track": &track_info,
                    "reason": "loadFailed"
                }));
                self.emit_event(json!({
                    "op": "event",
                    "type": "TrackExceptionEvent",
                    "guildId": self.guild_id,
                    "exception": {
                        "message": format!("Failed to resolve track URL: {e}"),
                        "severity": "fault",
                        "cause": "URL Resolution Error"
                    }
                }));
                return false;
            }
        };

        let is_sabr = protocol == "sabr";

        self.emit_event(json!({
            "op": "event",
            "type": "TrackStartEvent",
            "guildId": self.guild_id,
            "track": &track_info
        }));

        // SponsorBlock: fetch segments for YouTube tracks
        let sb_segments = if track_info.source_name == "youtube" {
            let categories = self
                .sponsorblock
                .get(&self.guild_id)
                .map(|v| v.clone())
                .unwrap_or_default();
            if !categories.is_empty() {
                let segments = match fetch_segments(&track_info.identifier, &categories).await {
                    Ok(segs) => segs,
                    Err(e) => {
                        warn!(target: "Worker", "SponsorBlock fetch failed: {} (guild: {})", e, self.guild_id);
                        Vec::new()
                    }
                };
                if !segments.is_empty() {
                    self.emit_event(json!({
                        "op": "event",
                        "type": crate::constants::gateway_events::SPONSORBLOCK_SEGMENTS_LOADED,
                        "guildId": self.guild_id,
                        "segments": segments.iter().map(|s| json!({
                            "start": s.start(),
                            "end": s.end(),
                            "category": s.category,
                            "uuid": s.uuid
                        })).collect::<Vec<_>>()
                    }));
                }
                segments
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let sb_segments = Arc::new(RwLock::new(sb_segments));

        // Fetch lyrics only if subscribed
        let subscribed_lyrics = {
            let subs = self.subscriptions.lock().await;
            subs.contains("lyrics")
        };
        let lyrics_lines = if subscribed_lyrics {
            match fetch_lyrics(&track_info.title, &track_info.author, None, Some(&track_info.identifier)).await {
                Ok(Some(data)) => {
                    if !data.synced_lyrics.is_empty() {
                        self.emit_event(json!({
                            "op": "event",
                            "type": "LyricsFound",
                            "guildId": self.guild_id,
                            "title": data.title,
                            "artist": data.artist,
                            "album": data.album,
                            "source": data.source,
                            "synced": true
                        }));
                        Some(data.synced_lyrics)
                    } else {
                        None
                    }
                }
                _ => {
                    self.emit_event(json!({
                        "op": "event",
                        "type": "LyricsNotFound",
                        "guildId": self.guild_id
                    }));
                    None
                }
            }
        } else {
            None
        };

        self.is_playing = true;
        self.paused.store(false, Ordering::SeqCst);

        let guild_id = self.guild_id.clone();
        let ws = self.ws_sender.clone();
        let paused = self.paused.clone();
        let position = self.position.clone();
        let track_info_clone = track_info.clone();
        let source_url = url.clone();
        let track_end = self.track_ended_notify.clone();
        let filters = self.current_filters.clone();
        let live_state = self.live_state.clone();

        let udp_socket = voice_session.udp_socket.clone();
        let ssrc = voice_session.ssrc;
        let addr = voice_session.address;
        let seq = voice_session.sequence;
        let ts = voice_session.timestamp;
        let key = voice_session.secret_key;
        let sb_segments = sb_segments.clone();
        let lyrics_lines = lyrics_lines.clone();
        let track_start_cfg = self.fade_config.track_start.clone();
        let track_end_cfg = self.fade_config.track_end.clone();
        let pause_cfg = self.fade_config.pause.clone();
        let resume_cfg = self.fade_config.resume.clone();
        let fade_enabled = self.fade_config.enabled;
        let stuck_threshold_ms = self.track_stuck_threshold_ms;
        let resample_quality = self.resample_quality.clone();
        let queue_state = self.queue_state.clone();
        let sources = self.sources.clone();
        let crossfade_config = self.crossfade_config.clone();
        let loudness_enabled = self.loudness_normalizer;
        let loudness_lookahead_ms = self.lookahead_ms;
        let loudness_gate = self.gate_threshold_lufs;
        let mixer = self.mixer.clone();
        let track_cancelled = Arc::new(AtomicBool::new(false));
        self.playback_cancelled = track_cancelled.clone();
        let plugin_manager = self.plugin_manager.clone();
        let sabr_additional = sabr_additional.clone();
        let video_id = track_info.identifier.clone();
        let is_sabr = is_sabr;
        let sabr_session_state = self.sabr_session_state.clone();

        let playback_handle = tokio::spawn(async move {
            if track_cancelled.load(Ordering::Relaxed) {
                track_end.notify_one();
                return;
            }
            let mut lyrics_index: usize = 0;
            Self::send_ws(&ws, json!({
                "op": "playerUpdate",
                "guildId": guild_id,
                "state": {
                    "time": 0u64,
                    "position": 0u64,
                    "connected": true,
                    "ping": -1i64
                }
            }));

            plugin_manager.on_track_start(&guild_id, &serde_json::to_value(&track_info_clone).unwrap_or_default()).await;

            let resample_quality = ResampleQuality::from_str(&resample_quality);
            let start_pos = position.load(Ordering::SeqCst);
            let mut pipeline = if is_sabr {
                match create_sabr_pipeline(&sabr_additional, &video_id, resample_quality, start_pos, &sabr_session_state).await {
                    Ok(p) => p,
                    Err(e) => {
                        error!(target: "Worker", "SABR pipeline init failed: {} (guild: {})", e, guild_id);
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "TrackEndEvent",
                            "guildId": guild_id,
                            "track": &track_info_clone,
                            "reason": "loadFailed"
                        }));
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "TrackExceptionEvent",
                            "guildId": guild_id,
                            "exception": {
                                "message": format!("SABR pipeline init failed: {e}"),
                                "severity": "fault",
                                "cause": "SABR Pipeline Error"
                            }
                        }));
                        if !track_cancelled.load(Ordering::Relaxed) {
                            track_end.notify_one();
                        }
                        return;
                    }
                }
            } else {
                match AudioPipeline::new(&source_url, Some(resample_quality)).await {
                Ok(p) => p,
                Err(e) => {
                    error!(target: "Worker", "Pipeline init failed: {} (guild: {})", e, guild_id);
                    Self::send_ws(&ws, json!({
                        "op": "event",
                        "type": "TrackEndEvent",
                        "guildId": guild_id,
                        "track": &track_info_clone,
                        "reason": "loadFailed"
                    }));
                    Self::send_ws(&ws, json!({
                        "op": "event",
                        "type": "TrackExceptionEvent",
                        "guildId": guild_id,
                        "exception": {
                            "message": format!("Pipeline init failed: {e}"),
                            "severity": "fault",
                            "cause": "Audio Pipeline Error"
                        }
                    }));
                    if !track_cancelled.load(Ordering::Relaxed) {
                        track_end.notify_one();
                    }
                    return;
                }
            }
            };

            // Seek to current position (handles seek commands)
            if start_pos > 0 && !is_sabr {
                if let Err(e) = pipeline.seek_to(start_pos) {
                    warn!(target: "Worker", "Seek to {}ms failed: {} (guild: {})", start_pos, e, guild_id);
                }
            }

            let channels = pipeline.channels();
            Self::send_ws(&ws, json!({
                "op": "event",
                "type": "StreamMetadata",
                "guildId": guild_id,
                "sampleRate": 48000u32,
                "channels": channels,
                "position": start_pos
            }));
            let mut opus_encoder = match opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio) {
                Ok(e) => e,
                Err(_e) => {
                    error!(target: "Worker", "Opus encoder init failed (guild: {})", guild_id);
                    Self::send_ws(&ws, json!({
                        "op": "event",
                        "type": "TrackExceptionEvent",
                        "guildId": guild_id,
                        "exception": {
                            "message": format!("Opus encoder init failed"),
                            "severity": "fault",
                            "cause": "Audio Pipeline Error"
                        }
                    }));
                    if !track_cancelled.load(Ordering::Relaxed) {
                        track_end.notify_one();
                    }
                    return;
                }
            };

            let mut loudness_norm = if loudness_enabled && loudness_lookahead_ms > 0 {
                Some(LoudnessNormalizer::new(
                    channels,
                    pipeline.sample_rate(),
                    loudness_lookahead_ms,
                    -14.0,
                    loudness_gate,
                ))
            } else {
                None
            };

            let mut layer_pipelines: HashMap<String, AudioPipeline> = HashMap::new();

            let mut volume_fade = VolumeFade::new();
            let mut tape_fade = TapeFade::new();
            let mut tape_buffer: Vec<f32> = Vec::new();
            let frame_samples = 960 * channels as usize;
            let mut was_paused = false;
            let channels_val = channels;
            let mut crossfade_buffer: Vec<f32> = Vec::new();

            if fade_enabled && track_start_cfg.duration > 0 {
                match track_start_cfg.kind.as_str() {
                    "tape" => tape_fade.trigger(
                        TapeAction::Start,
                        track_start_cfg.duration as f32,
                        &track_start_cfg.curve,
                    ),
                    _ => volume_fade.trigger(
                        1.0,
                        track_start_cfg.duration as f32,
                        &track_start_cfg.curve,
                    ),
                }
            }

            let mut sess = VoiceSession {
                udp_socket,
                ssrc,
                secret_key: key,
                sequence: seq,
                timestamp: ts,
                address: addr,
                encryption_mode: crate::player::voice::EncryptionMode::XSalsa20Poly1305,
            };

            // Track length for scheduled end fade
            let track_length_ms = track_info_clone.length as u64;
            let track_end_fade_start_ms = if fade_enabled && track_end_cfg.duration > 0 && track_length_ms > 0 {
                track_length_ms.saturating_sub(track_end_cfg.duration as u64)
            } else {
                u64::MAX
            };
            let mut track_end_fade_triggered = false;

            let mut last_update = Instant::now();
            let mut last_frame_time = Instant::now();
            let mut stuck_emitted = false;
            let mut sent_count: u64 = 0;
            let mut nulled_count: u64 = 0;
            let mut deficit_count: u64 = 0;

            loop {
                let p = paused.load(Ordering::SeqCst);
                if p && !was_paused && fade_enabled && pause_cfg.duration > 0 {
                    match pause_cfg.kind.as_str() {
                        "tape" => tape_fade.trigger(
                            TapeAction::Stop,
                            pause_cfg.duration as f32,
                            &pause_cfg.curve,
                        ),
                        _ => volume_fade.trigger(
                            0.0,
                            pause_cfg.duration as f32,
                            &pause_cfg.curve,
                        ),
                    }
                }
                if !p && was_paused && fade_enabled && resume_cfg.duration > 0 {
                    match resume_cfg.kind.as_str() {
                        "tape" => tape_fade.trigger(
                            TapeAction::Start,
                            resume_cfg.duration as f32,
                            &resume_cfg.curve,
                        ),
                        _ => volume_fade.trigger(
                            1.0,
                            resume_cfg.duration as f32,
                            &resume_cfg.curve,
                        ),
                    }
                }
                was_paused = p;

                // Scheduled track end fade (NodeLink's trackEndSchedule)
                if !track_end_fade_triggered
                    && fade_enabled
                    && track_end_cfg.duration > 0
                    && track_length_ms > 0
                {
                    let pos = position.load(Ordering::SeqCst);
                    if pos >= track_end_fade_start_ms {
                        track_end_fade_triggered = true;
                        match track_end_cfg.kind.as_str() {
                            "tape" => tape_fade.trigger(
                                TapeAction::Stop,
                                track_end_cfg.duration as f32,
                                &track_end_cfg.curve,
                            ),
                            _ => volume_fade.trigger(
                                0.0,
                                track_end_cfg.duration as f32,
                                &track_end_cfg.curve,
                            ),
                        }
                    }
                }

                if p && !tape_fade.is_active() && !volume_fade.active {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    last_update = Instant::now();
                    continue;
                }

                // TrackStuckEvent detection
                if stuck_threshold_ms > 0 && last_frame_time.elapsed() >= Duration::from_millis(stuck_threshold_ms) && !stuck_emitted {
                    Self::send_ws(&ws, json!({
                        "op": "event",
                        "type": "TrackStuckEvent",
                        "guildId": guild_id,
                        "thresholdMs": stuck_threshold_ms,
                    }));
                    stuck_emitted = true;
                    last_frame_time = Instant::now();
                    plugin_manager.on_track_stuck(&guild_id, &serde_json::to_value(&track_info_clone).unwrap_or_default(), stuck_threshold_ms).await;
                }

                let mut pcm = match pipeline.next_pcm_frame() {
                    Ok(Some(f)) => {
                        // Ring buffer for crossfade (raw PCM before filters)
                        if crossfade_config.enabled && crossfade_config.duration > 0 {
                            let max_samp = ((crossfade_config.duration / 20) as usize) * frame_samples;
                            crossfade_buffer.extend_from_slice(&f);
                            if crossfade_buffer.len() > max_samp {
                                crossfade_buffer.drain(..crossfade_buffer.len() - max_samp);
                            }
                        }
                        last_frame_time = Instant::now();
                        stuck_emitted = false;
                        f
                    }
                    Ok(None) => {
                        info!(target: "Worker", "Track ended naturally (guild: {})", guild_id);

                        // Crossfade: check for next track
                        let next_for_crossfade = if crossfade_config.enabled && crossfade_config.duration > 0 && !crossfade_buffer.is_empty() {
                            let mut qs = queue_state.lock().await;
                            let next = qs.next_track();
                            if let Some(ref enc) = next {
                                qs.current_encoded = Some(enc.clone());
                            }
                            next
                        } else {
                            None
                        };

                        if let Some(ref next_enc) = next_for_crossfade {
                            let next_track = match decode_track(next_enc) {
                                Ok(t) => Some(t),
                                Err(e) => {
                                    error!(target: "Worker", "Crossfade: decode error: {} (guild: {})", e, guild_id);
                                    None
                                }
                            };

                            if let Some(ref next_track) = next_track {
                                let next_url = match sources.get_track_url(&next_track.info).await {
                                    Ok(r) => r.url,
                                    Err(_) => None,
                                };

                                if let Some(ref next_url) = next_url {
                                    info!(target: "Worker", "Crossfade: transitioning to next track (guild: {})", guild_id);

                                    Self::send_ws(&ws, json!({
                                        "op": "event",
                                        "type": "TrackEndEvent",
                                        "guildId": guild_id,
                                        "track": &track_info_clone,
                                        "reason": "finished"
                                    }));
                                    Self::send_ws(&ws, json!({
                                        "op": "event",
                                        "type": "TrackStartEvent",
                                        "guildId": guild_id,
                                        "track": &next_track.info
                                    }));

                                    let mut pipeline_data: Option<AudioPipeline> = None;
                                    match AudioPipeline::new(next_url, Some(resample_quality)).await {
                                        Ok(p) => pipeline_data = Some(p),
                                        Err(e) => error!(target: "Worker", "Crossfade: pipeline error: {} (guild: {})", e, guild_id),
                                    }

                                    if let Some(ref mut next_pipeline) = pipeline_data {
                                        let crossfade_ms = crossfade_config.duration;
                                        let fade_frames = (crossfade_ms / 20) as usize;
                                        let avail_frames = crossfade_buffer.len() / frame_samples;
                                        let use_frames = fade_frames.min(avail_frames);

                                        let mut old_fade = VolumeFade::new();
                                        old_fade.trigger(0.0, crossfade_ms as f32, &crossfade_config.curve);
                                        let mut new_fade = VolumeFade::new();
                                        new_fade.trigger(1.0, crossfade_ms as f32, &crossfade_config.curve);

                                        for i in 0..use_frames {
                                            let start = i * frame_samples;
                                            let end = start + frame_samples;
                                            let mut old_chunk = crossfade_buffer[start..end].to_vec();
                                            let mut new_chunk = next_pipeline.next_pcm_frame()
                                                .ok()
                                                .flatten()
                                                .unwrap_or_else(|| vec![0.0; frame_samples]);

                                            {
                                                let mut fc = filters.write().await;
                                                fc.process(&mut old_chunk, channels_val);
                                                fc.process(&mut new_chunk, channels_val);
                                            }

                                            old_fade.process(&mut old_chunk, 20.0);
                                            new_fade.process(&mut new_chunk, 20.0);

                                            for s in 0..frame_samples {
                                                old_chunk[s] += new_chunk[s];
                                            }

                                            if let Ok(enc) = audio_pipeline::encode_opus_frame(&old_chunk, &mut opus_encoder) {
                                                if sess.send_opus_frame(&enc).await.is_err() {
                                                    error!(target: "Worker", "Crossfade: send error (guild: {})", guild_id);
                                                    deficit_count += 1;
                                                    break;
                                                }
                                                sent_count += 1;
                                            }
                                            position.fetch_add(20, Ordering::SeqCst);
                                        }
                                    }

                                    if let Some(next_pipeline) = pipeline_data {
                                        pipeline = next_pipeline;
                                        tape_fade = TapeFade::new();
                                        tape_buffer.clear();
                                        volume_fade = VolumeFade::new();
                                        crossfade_buffer.clear();

                                        if fade_enabled && track_start_cfg.duration > 0 {
                                            match track_start_cfg.kind.as_str() {
                                                "tape" => tape_fade.trigger(
                                                    TapeAction::Start,
                                                    track_start_cfg.duration as f32,
                                                    &track_start_cfg.curve,
                                                ),
                                                _ => volume_fade.trigger(
                                                    1.0,
                                                    track_start_cfg.duration as f32,
                                                    &track_start_cfg.curve,
                                                ),
                                            }
                                        }

                                        continue;
                                    }
                                }
                            }
                        }

                        // Normal end-of-track (fallthrough when crossfade not applicable)
                        // Skip if scheduled fade already handled it fully
                        if fade_enabled && track_end_cfg.duration > 0 && !track_end_fade_triggered {
                            let end_kind = track_end_cfg.kind.clone();
                            let end_curve = track_end_cfg.curve.clone();
                            let end_dur = track_end_cfg.duration as f32;
                            match end_kind.as_str() {
                                "tape" => {
                                    tape_fade.trigger(TapeAction::Stop, end_dur, &end_curve);
                                    loop {
                                        if let Ok(Some(remaining)) = pipeline.next_pcm_frame() {
                                            let mut t_out = Vec::new();
                                            tape_fade.process(&remaining, &mut t_out, channels_val, 20.0);
                                            tape_buffer.extend(t_out);
                                        }
                                        while tape_buffer.len() >= frame_samples {
                                            let mut fr: Vec<f32> = tape_buffer.drain(..frame_samples).collect();
                                            volume_fade.process(&mut fr, 20.0);
                                            if let Ok(enc) = audio_pipeline::encode_opus_frame(&fr, &mut opus_encoder) {
                                                if sess.send_opus_frame(&enc).await.is_ok() {
                                                    sent_count += 1;
                                                } else {
                                                    deficit_count += 1;
                                                }
                                            }
                                        }
                                        if !tape_fade.is_active() && tape_buffer.len() < frame_samples {
                                            break;
                                        }
                                        tokio::time::sleep(Duration::from_millis(5)).await;
                                    }
                                }
                                _ => {
                                    volume_fade.trigger(0.0, end_dur, &end_curve);
                                    let fade_frames = ((end_dur / 20.0).ceil() as usize).max(1);
                                    for _ in 0..fade_frames {
                                        if let Ok(Some(mut remaining)) = pipeline.next_pcm_frame() {
                                            volume_fade.process(&mut remaining, 20.0);
                                            if let Ok(frame) = audio_pipeline::encode_opus_frame(&remaining, &mut opus_encoder) {
                                                if sess.send_opus_frame(&frame).await.is_ok() {
                                                    sent_count += 1;
                                                } else {
                                                    deficit_count += 1;
                                                }
                                            }
                                        } else {
                                            let mut silence = vec![0.0f32; frame_samples];
                                            volume_fade.process(&mut silence, 20.0);
                                            if let Ok(frame) = audio_pipeline::encode_opus_frame(&silence, &mut opus_encoder) {
                                                if sess.send_opus_frame(&frame).await.is_ok() {
                                                    sent_count += 1;
                                                } else {
                                                    deficit_count += 1;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let pos = position.load(Ordering::SeqCst);
                        Self::send_ws(&ws, json!({
                            "op": "playerUpdate",
                            "guildId": guild_id,
                            "state": {
                                "time": pos,
                                "position": pos,
                                "connected": true,
                                "ping": -1i64
                            }
                        }));
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "TrackEndEvent",
                            "guildId": guild_id,
                            "track": &track_info_clone,
                            "reason": "finished"
                        }));
                        plugin_manager.on_track_end(&guild_id, &serde_json::to_value(&track_info_clone).unwrap_or_default(), "finished").await;
                        break;
                    }
                    Err(e) => {
                        error!(target: "Worker", "Pipeline error: {} (guild: {})", e, guild_id);
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "TrackExceptionEvent",
                            "guildId": guild_id,
                            "exception": {
                                "message": format!("Pipeline error: {e}"),
                                "severity": "fault",
                                "cause": "Audio Pipeline Error"
                            }
                        }));
                        break;
                    }
                };

                // Mixer layers: read PCM from each active layer and mix into output
                {
                    let mixer_layers = mixer.lock().await.layers().to_vec();
                    let has_solo = mixer_layers.iter().any(|l| l.solo);
                    for layer in &mixer_layers {
                        if layer.mute { continue; }
                        if layer.url.is_none() { continue; }
                        let active_vol = if has_solo && !layer.solo {
                            0.0
                        } else {
                            layer.volume
                        };
                        if active_vol == 0.0 { continue; }

                        // Create pipeline on first use
                        if !layer_pipelines.contains_key(&layer.id) {
                            if let Some(ref url) = layer.url {
                                match AudioPipeline::new(url, Some(resample_quality)).await {
                                    Ok(p) => { layer_pipelines.insert(layer.id.clone(), p); }
                                    Err(_) => continue,
                                }
                            }
                        }

                        if let Some(pipe) = layer_pipelines.get_mut(&layer.id) {
                            if let Ok(Some(layer_pcm)) = pipe.next_pcm_frame() {
                                AudioMixer::mix_into(
                                    &mut pcm,
                                    &layer_pcm,
                                    active_vol,
                                    layer.pan,
                                    channels_val,
                                );
                            } else {
                                // Source ended; remove pipeline (layer config stays)
                                layer_pipelines.remove(&layer.id);
                            }
                        }
                    }
                    // Remove pipelines for layers that no longer exist
                    let active_ids: HashSet<&str> = mixer_layers.iter().map(|l| l.id.as_str()).collect();
                    layer_pipelines.retain(|id, _| active_ids.contains(id.as_str()));
                }

                // Loudness normalizer (before filters)
                if let Some(ref mut ln) = loudness_norm {
                    ln.process(&mut pcm);
                }

                {
                    let mut fc = filters.write().await;
                    fc.process(&mut pcm, channels_val);
                }

                // Feed through TapeFade, accumulate output
                let mut tape_out = Vec::new();
                tape_fade.process(&pcm, &mut tape_out, channels_val, 20.0);
                tape_buffer.extend(tape_out);

                let pos_ms = position.load(Ordering::SeqCst);

                // Drain and encode complete frames
                while tape_buffer.len() >= frame_samples {
                    let mut frame: Vec<f32> = tape_buffer.drain(..frame_samples).collect();
                    volume_fade.process(&mut frame, 20.0);

                    let encoded = match audio_pipeline::encode_opus_frame(&frame, &mut opus_encoder) {
                        Ok(f) => f,
                        Err(e) => {
                            error!(target: "Worker", "Opus encode error: {} (guild: {})", e, guild_id);
                            break;
                        }
                    };

                    if let Err(e) = sess.send_opus_frame(&encoded).await {
                        error!(target: "Worker", "Send error: {} (guild: {})", e, guild_id);
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "TrackExceptionEvent",
                            "guildId": guild_id,
                            "exception": {
                                "message": format!("Send error: {e}"),
                                "severity": "fault",
                                "cause": "Voice Send Error"
                            }
                        }));
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "WebSocketClosedEvent",
                            "guildId": guild_id,
                            "code": 4014,
                            "reason": "Disconnected",
                            "byRemote": true
                        }));
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": crate::constants::gateway_events::CONNECTION_STATUS,
                            "guildId": guild_id,
                            "state": "DISCONNECTED",
                            "reason": "Voice send failure"
                        }));
                        deficit_count += 1;
                    } else {
                        sent_count += 1;
                        plugin_manager.on_audio_packet(&guild_id, &encoded).await;
                    }

                    position.fetch_add(20, Ordering::SeqCst);
                }

                // SponsorBlock auto-skip
                {
                    let segments = sb_segments.read().await;
                    if !segments.is_empty() {
                        let pos_secs = pos_ms as f64 / 1000.0;
                        let seg_to_skip = segments.iter().find(|s| pos_secs >= s.start() && pos_secs < s.end()).cloned();
                        if let Some(seg) = seg_to_skip {
                            let skip_to_ms = (seg.end() * 1000.0) as u64;
                            info!(target: "Worker", "SponsorBlock: skipping {} (category: {}) (guild: {})", seg.category, seg.category, guild_id);
                            tape_buffer.clear();
                            let skip_to_frames = skip_to_ms / 20;
                            let current_frames = pos_ms / 20;
                            let frames_to_skip = skip_to_frames.saturating_sub(current_frames);
                            for _ in 0..frames_to_skip {
                                if let Ok(Some(_)) = pipeline.next_pcm_frame() {
                                    // discard
                                } else {
                                    break;
                                }
                            }
                            position.store(skip_to_ms, Ordering::SeqCst);
                            let skipped_ms = ((seg.end() - (pos_ms as f64 / 1000.0)) * 1000.0) as u64;
                            Self::send_ws(&ws, json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::SPONSORBLOCK_SEGMENT_SKIPPED,
                                "guildId": guild_id,
                                "segment": {
                                    "start": seg.start(),
                                    "end": seg.end(),
                                    "category": seg.category,
                                    "uuid": seg.uuid
                                },
                                "skippedMs": skipped_ms
                            }));
                            last_update = Instant::now();
                            continue;
                        }
                    }
                }

                // Lyrics line emission
                if let Some(ref lines) = lyrics_lines {
                    let pos_secs = pos_ms as f64 / 1000.0;
                    while lyrics_index < lines.len() && lines[lyrics_index].time <= pos_secs {
                        Self::send_ws(&ws, json!({
                            "op": "event",
                            "type": "LyricsLine",
                            "guildId": guild_id,
                            "line": lines[lyrics_index].text,
                            "lineNumber": lyrics_index as u64 + 1,
                            "timestamp": lines[lyrics_index].time
                        }));
                        lyrics_index += 1;
                    }
                }

                if last_update.elapsed() >= Duration::from_secs(1) {
                    Self::send_ws(&ws, json!({
                        "op": "playerUpdate",
                        "guildId": guild_id,
                        "state": {
                            "time": pos_ms,
                            "position": pos_ms,
                            "connected": true,
                            "ping": -1i64
                        }
                    }));
                    {
                        let mut ls = live_state.write().await;
                        ls.position = pos_ms;
                        ls.connected = true;
                        ls.ping = -1;
                        ls.frames_sent = ls.frames_sent.wrapping_add(sent_count);
                        ls.frames_nulled = ls.frames_nulled.wrapping_add(nulled_count);
                        ls.frames_deficit = ls.frames_deficit.wrapping_add(deficit_count);
                        sent_count = 0;
                        nulled_count = 0;
                        deficit_count = 0;
                    }
                    plugin_manager.on_player_update(&guild_id, &serde_json::json!({
                        "time": pos_ms,
                        "position": pos_ms,
                        "connected": true,
                        "ping": -1i64
                    })).await;
                    last_update = Instant::now();
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            if !track_cancelled.load(Ordering::Relaxed) {
                track_end.notify_one();
            }
        });

        // Monitor for panics in the playback task
        let monitor_ws = self.ws_sender.clone();
        let monitor_guild = self.guild_id.clone();
        let monitor = tokio::spawn(async move {
            if let Err(e) = playback_handle.await {
                if e.is_panic() {
                    Self::send_ws(&monitor_ws, json!({
                        "op": "event",
                        "type": "WorkerFailedEvent",
                        "guildId": monitor_guild,
                        "message": "Playback worker panicked"
                    }));
                }
            }
        });

        self.playback_task = Some(monitor);
        true
    }

    pub async fn run(mut self) {
        info!(target: "Worker", "Spawned for Guild: {}", self.guild_id);

        let mut voice_session: Option<VoiceSession> = None;
        let track_end = self.track_ended_notify.clone();

        loop {
            tokio::select! {
                cmd = self.rx.recv() => {
                    let Some(cmd) = cmd else {
                        break;
                    };

                    match cmd {
                        WorkerCommand::VoiceUpdate {
                            session_id,
                            user_id,
                            token,
                            endpoint,
                        } => {
                            info!(target: "Worker", "Connecting to Discord voice: {} (guild: {})", endpoint, self.guild_id);
                            let conn_session_id = session_id.clone();
                            let conn_token = token.clone();
                            let conn_endpoint = endpoint.clone();
                            let mut conn = VoiceConnection::new(
                                self.guild_id.clone(),
                                user_id,
                                session_id,
                                token,
                                endpoint,
                            );
                            match conn.connect().await {
                                Ok(session_arc) => {
                                    let guard = session_arc.lock().await;
                                    let session = VoiceSession {
                                        udp_socket: guard.udp_socket.clone(),
                                        ssrc: guard.ssrc,
                                        secret_key: guard.secret_key,
                                        sequence: guard.sequence,
                                        timestamp: guard.timestamp,
                                        address: guard.address,
                                        encryption_mode: guard.encryption_mode,
                                    };
                                    drop(guard);
                                    {
                                        let mut ls = self.live_state.write().await;
                                        ls.connected = true;
                                        ls.voice.session_id = Some(conn_session_id);
                                        ls.voice.token = Some(conn_token);
                                        ls.voice.endpoint = Some(conn_endpoint);
                                    }
                                    self.emit_event(json!({
                                        "op": "event",
                                        "type": crate::constants::gateway_events::CONNECTION_STATUS,
                                        "guildId": self.guild_id,
                                        "state": "CONNECTED"
                                    }));
                                    voice_session = Some(session);
                                    info!(target: "Worker", "Voice session established (guild: {})", self.guild_id);
                                }
                                Err(e) => {
                                    error!(target: "Worker", "Voice connect failed: {} (guild: {})", e, self.guild_id);
                                    self.emit_event(json!({
                                        "op": "event",
                                        "type": crate::constants::gateway_events::CONNECTION_STATUS,
                                        "guildId": self.guild_id,
                                        "state": "DISCONNECTED",
                                        "reason": format!("Connect failed: {e}")
                                    }));
                                }
                            }
                        }
                        WorkerCommand::Play { encoded_track, no_replace } => {
                            info!(target: "Worker", "Play command received (guild: {})", self.guild_id);

                            if self.destroying || self.is_stopping {
                                continue;
                            }

                            if no_replace && self.is_playing {
                                info!(target: "Worker", "noReplace=true, enqueueing (guild: {})", self.guild_id);
                                self.queue_state.lock().await.queue.push(encoded_track);
                                continue;
                            }

                            self.is_updating_track = true;
                            if self.playback_task.is_some() {
                                if let Some(ref track) = self.current_track {
                                    self.emit_event(json!({
                                        "op": "event",
                                        "type": "TrackEndEvent",
                                        "guildId": self.guild_id,
                                        "track": track,
                                        "reason": "replaced"
                                    }));
                                }
                                self.abort_current_playback();
                            }

                            let session = match voice_session.as_mut() {
                                Some(s) => s,
                                None => {
                                    warn!(target: "Worker", "No voice session — send voiceUpdate first (guild: {})", self.guild_id);
                                    self.is_updating_track = false;
                                    continue;
                                }
                            };

                            self.position.store(0, Ordering::SeqCst);
                            self.stuck_recovery_count = 0;
                            self.stuck_time_ms = 0;
                            self.last_position = 0;
                            self.start_track(&encoded_track, session).await;
                            self.is_updating_track = false;
                        }
                        WorkerCommand::Pause(state) => {
                            if self.destroying || self.status == PlayerStatus::Idle {
                                continue;
                            }
                            self.paused.store(state, Ordering::SeqCst);
                            self.is_playing = !state;
                            self.status = if state { PlayerStatus::Paused } else { PlayerStatus::Playing };
                            info!(target: "Worker", "Paused: {} (guild: {})", state, self.guild_id);
                            {
                                let mut ls = self.live_state.write().await;
                                ls.paused = state;
                            }
                            self.emit_event_or_queue(json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::PAUSE,
                                "guildId": self.guild_id,
                                "paused": state
                            }));
                        }
                        WorkerCommand::Volume(vol) => {
                            if self.destroying {
                                continue;
                            }
                            info!(target: "Worker", "Volume set to {} (guild: {})", vol, self.guild_id);
                            {
                                let mut ls = self.live_state.write().await;
                                ls.volume = vol as u32;
                            }
                            self.emit_event_or_queue(json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::VOLUME_CHANGED,
                                "guildId": self.guild_id,
                                "volume": vol
                            }));
                        }
                        WorkerCommand::Seek(position_ms) => {
                            if self.destroying || self.is_seeking || self.status == PlayerStatus::Idle {
                                continue;
                            }
                            self.is_seeking = true;
                            info!(target: "Worker", "Seek to {}ms (guild: {})", position_ms, self.guild_id);
                            self.emit_event_or_queue(json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::SEEK,
                                "guildId": self.guild_id,
                                "position": position_ms
                            }));
                            let encoded = self.current_track_encoded.clone().unwrap_or_default();
                            self.abort_current_playback();

                            self.position.store(position_ms, Ordering::SeqCst);

                            if let Some(session) = voice_session.as_mut() {
                                self.start_track(&encoded, session).await;
                            } else {
                                warn!(target: "Worker", "Seek: no voice session (guild: {})", self.guild_id);
                            }
                            self.is_seeking = false;
                        }
                        WorkerCommand::Filters(filters) => {
                            if self.destroying {
                                continue;
                            }
                            info!(target: "Worker", "Filters updated (guild: {})", self.guild_id);
                            {
                                let mut fc = self.current_filters.write().await;
                                fc.update_from_json(&filters);
                            }
                            self.emit_event_or_queue(json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::FILTERS_CHANGED,
                                "guildId": self.guild_id,
                                "filters": filters
                            }));
                        }
                        WorkerCommand::Stop => {
                            if self.destroying || self.is_stopping || self.status == PlayerStatus::Idle {
                                continue;
                            }
                            self.is_stopping = true;
                            self.is_playing = false;
                            self.status = PlayerStatus::Stopped;
                            if self.playback_task.is_some() {
                                if let Some(ref track) = self.current_track {
                                    self.emit_event_or_queue(json!({
                                        "op": "event",
                                        "type": "TrackEndEvent",
                                        "guildId": self.guild_id,
                                        "track": track,
                                        "reason": "stopped"
                                    }));
                                }
                                self.abort_current_playback();
                            }
                            info!(target: "Worker", "Playback stopped (guild: {})", self.guild_id);
                            self.is_stopping = false;
                        }
                        WorkerCommand::NextTrack => {
                            if self.destroying || self.is_updating_track {
                                continue;
                            }
                            info!(target: "Worker", "NextTrack command (guild: {})", self.guild_id);
                            // Check preloaded next track first (gapless)
                            self.is_updating_track = true;
                            self.abort_current_playback();
                            if let Some(session) = voice_session.as_mut() {
                                let queued = self.queue_state.lock().await.next_track();
                                let next = self.next_track_encoded.take().or(queued);
                                if let Some(next) = next {
                                    self.position.store(0, Ordering::SeqCst);
                                    self.stuck_recovery_count = 0;
                                    self.stuck_time_ms = 0;
                                    self.last_position = 0;
                                    self.start_track(&next, session).await;
                                } else {
                                    self.is_playing = false;
                                    self.status = PlayerStatus::Idle;
                                }
                            }
                            self.is_updating_track = false;
                        }
                        WorkerCommand::Preload { encoded_track } => {
                            info!(target: "Worker", "Preload track for gapless (guild: {})", self.guild_id);
                            self.next_track_encoded = Some(encoded_track);
                        }
                        WorkerCommand::Repeat(mode) => {
                            self.queue_state.lock().await.repeat = mode;
                            info!(target: "Worker", "Repeat mode set to {:?} (guild: {})", mode, self.guild_id);
                        }
                        WorkerCommand::Shuffle => {
                            {
                                let mut qs = self.queue_state.lock().await;
                                use rand::seq::SliceRandom;
                                let mut rng = rand::thread_rng();
                                qs.queue.shuffle(&mut rng);
                            }
                            info!(target: "Worker", "Queue shuffled (guild: {})", self.guild_id);
                        }
                        WorkerCommand::MixerAddLayer { name, volume, pan } => {
                            let id = {
                                let mut m = self.mixer.lock().await;
                                m.add_layer(&name, volume, pan)
                            };
                            info!(target: "Worker", "Mixer layer added: {} (guild: {})", id, self.guild_id);
                            let mixer_json = self.mixer.lock().await.to_json();
                            self.live_state.write().await.mixer = Some(mixer_json.clone());
                            self.emit_event(json!({
                                "op": "event",
                                "type": "MixerState",
                                "guildId": self.guild_id,
                                "layers": mixer_json
                            }));
                        }
                        WorkerCommand::MixerRemoveLayer { layer_id } => {
                            let removed = self.mixer.lock().await.remove_layer(&layer_id);
                            if removed {
                                info!(target: "Worker", "Mixer layer removed: {} (guild: {})", layer_id, self.guild_id);
                            }
                            let mixer_json = self.mixer.lock().await.to_json();
                            self.live_state.write().await.mixer = Some(mixer_json.clone());
                            self.emit_event(json!({
                                "op": "event",
                                "type": "MixerState",
                                "guildId": self.guild_id,
                                "layers": mixer_json
                            }));
                        }
                        WorkerCommand::MixerUpdateLayer { layer_id, name, volume, pan, mute, solo } => {
                            self.mixer.lock().await.update_layer(
                                &layer_id, name, volume, pan, mute, solo, None,
                            );
                            info!(target: "Worker", "Mixer layer updated: {} (guild: {})", layer_id, self.guild_id);
                            let mixer_json = self.mixer.lock().await.to_json();
                            self.live_state.write().await.mixer = Some(mixer_json.clone());
                            self.emit_event(json!({
                                "op": "event",
                                "type": "MixerState",
                                "guildId": self.guild_id,
                                "layers": mixer_json
                            }));
                        }
                        WorkerCommand::MixerSetUrl { layer_id, url } => {
                            self.mixer.lock().await.update_layer(
                                &layer_id, None, None, None, None, None, url,
                            );
                            info!(target: "Worker", "Mixer layer URL set: {} (guild: {})", layer_id, self.guild_id);
                            let mixer_json = self.mixer.lock().await.to_json();
                            self.live_state.write().await.mixer = Some(mixer_json.clone());
                            self.emit_event(json!({
                                "op": "event",
                                "type": "MixerState",
                                "guildId": self.guild_id,
                                "layers": mixer_json
                            }));
                        }
                        WorkerCommand::MixerList => {
                            let mixer_json = self.mixer.lock().await.to_json();
                            self.live_state.write().await.mixer = Some(mixer_json.clone());
                            self.emit_event(json!({
                                "op": "event",
                                "type": "MixerState",
                                "guildId": self.guild_id,
                                "layers": mixer_json
                            }));
                        }
                        WorkerCommand::Subscribe { topic } => {
                            info!(target: "Worker", "Subscribing to {} (guild: {})", topic, self.guild_id);
                            self.subscriptions.lock().await.insert(topic);
                        }
                        WorkerCommand::Unsubscribe { topic } => {
                            info!(target: "Worker", "Unsubscribing from {} (guild: {})", topic, self.guild_id);
                            self.subscriptions.lock().await.remove(&topic);
                        }
                        WorkerCommand::Destroy => {
                            info!(target: "Worker", "Destroying worker (guild: {})", self.guild_id);
                            if let Some(ref track) = self.current_track {
                                self.emit_event(json!({
                                    "op": "event",
                                    "type": "TrackEndEvent",
                                    "guildId": self.guild_id,
                                    "track": track,
                                    "reason": "cleanup"
                                }));
                            }
                            self.abort_current_playback();
                            self.emit_event(json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::CONNECTION_STATUS,
                                "guildId": self.guild_id,
                                "state": "DISCONNECTED",
                                "reason": "Player destroyed"
                            }));
                            self.emit_event(json!({
                                "op": "event",
                                "type": crate::constants::gateway_events::PLAYER_DESTROYED,
                                "guildId": self.guild_id
                            }));
                            self.player_states.remove(&self.guild_id);
                            break;
                        }
                    }
                }
                _ = track_end.notified() => {
                    if let Some(session) = voice_session.as_mut() {
                        // Check preloaded next track first (gapless)
                        let queued = self.queue_state.lock().await.next_track();
                        let next = self.next_track_encoded.take().or(queued);
                        if let Some(next) = next {
                            self.position.store(0, Ordering::SeqCst);
                            self.stuck_recovery_count = 0;
                            self.stuck_time_ms = 0;
                            self.last_position = 0;
                            self.start_track(&next, session).await;
                        } else {
                            self.is_playing = false;
                            self.status = PlayerStatus::Idle;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    self.check_stuck(voice_session.as_mut()).await;
                }
            }
        }
    }
}

async fn create_sabr_pipeline(
    additional_data: &Option<serde_json::Value>,
    _video_id: &str,
    resample_quality: ResampleQuality,
    start_time: u64,
    sabr_session_state: &Arc<std::sync::Mutex<Option<crate::sources::youtube_sabr::PreviousSessionState>>>,
) -> anyhow::Result<AudioPipeline> {
    let sabr_cfg = additional_data.as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing SABR additional data"))?;

    let server_url = sabr_cfg["serverAbrStreamingUrl"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing serverAbrStreamingUrl"))?;
    let client_name = sabr_cfg["clientName"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Missing clientName"))? as i32;
    let client_version = sabr_cfg["clientVersion"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing clientVersion"))?;

    let mut formats: Vec<crate::sources::youtube_sabr::FormatEntry> = Vec::new();
    if let Some(arr) = sabr_cfg["formats"].as_array() {
        for f in arr {
            let itag = f["itag"].as_i64().unwrap_or(0) as i32;
            if itag == 0 { continue; }
            formats.push(crate::sources::youtube_sabr::FormatEntry {
                itag,
                mime_type: f["mimeType"].as_str().map(|s| s.to_string()),
                xtags: f["xtags"].as_str().map(|s| s.to_string()),
                last_modified: f["lastModified"].as_str().map(|s| s.to_string()),
                audio_track_id: f["audioTrackId"].as_str().map(|s| s.to_string()),
                bitrate: f["bitrate"].as_i64().map(|v| v as i32),
            });
        }
    }

    // Extract optional PoToken and decode from base64
    let po_token = sabr_cfg["poToken"]
        .as_str()
        .and_then(|s| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).ok()
        });

    // Extract optional visitor data
    let visitor_data = sabr_cfg["visitorData"]
        .as_str()
        .map(|s| s.to_string());

    // Extract optional ustreamer config and decode from base64
    let video_playback_ustreamer_config = sabr_cfg["ustreamerConfig"]
        .as_str()
        .and_then(|s| {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.as_bytes()).ok()
        });

    let best_audio = formats.iter()
        .find(|f| f.mime_type.as_deref().map_or(false, |m| m.contains("audio")))
        .or_else(|| formats.first())
        .map(|f| f.itag)
        .unwrap_or(251);

    // Read previous session state from the shared handle
    let previous_session = {
        let mut guard = sabr_session_state.lock().unwrap();
        guard.take()
    };

    let config = crate::sources::youtube_sabr::SabrStreamConfig {
        video_id: _video_id.to_string(),
        server_abr_streaming_url: Some(server_url.to_string()),
        video_playback_ustreamer_config,
        client_info: Some(crate::sources::youtube_sabr::ClientInfoMsg {
            client_name,
            client_version: client_version.to_string(),
        }),
        formats,
        po_token,
        visitor_data,
        start_time: start_time as i64,
        user_agent: None,
        previous_session,
        access_token: None,
    };

    let http_client = reqwest::Client::new();
    let mut stream = crate::sources::youtube_sabr::SabrStream::new(http_client, config);
    let audio_rx = stream.audio_rx
        .take()
        .ok_or_else(|| anyhow::anyhow!("SABR audio receiver already taken"))?;

    let state_saver = sabr_session_state.clone();
    tokio::spawn(async move {
        stream.start(best_audio).await;
        *state_saver.lock().unwrap() = Some(stream.get_session_state());
    });

    AudioPipeline::from_channel(audio_rx, Some(resample_quality)).await
}
