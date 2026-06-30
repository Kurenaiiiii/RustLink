use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatsSnapshot {
    pub players: u32,
    pub playing_players: u32,
    pub uptime_ms: u64,
    pub memory: MemoryStats,
    pub cpu: CpuStats,
    pub frame_stats: FrameStats,
    pub api: ApiStats,
    pub sources: HashMap<String, SourceStats>,
    pub playback: PlaybackStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStats {
    pub free: u64,
    pub used: u64,
    pub allocated: u64,
    pub reservable: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CpuStats {
    pub cores: u32,
    pub system_load: f64,
    pub nodelink_load: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrameStats {
    pub sent: u64,
    pub nulled: u64,
    pub deficit: u64,
    pub expected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiStats {
    pub requests: HashMap<String, u64>,
    pub errors: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceStats {
    pub success: u64,
    pub failure: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackStats {
    pub events: HashMap<String, u64>,
}

struct InnerStats {
    players: u32,
    playing_players: u32,
    start_time: Instant,
    api_requests: HashMap<String, u64>,
    api_errors: HashMap<String, u64>,
    sources: HashMap<String, SourceStats>,
    playback_events: HashMap<String, u64>,
    frames_sent: u64,
    frames_nulled: u64,
    frames_deficit: u64,
    frames_expected: u64,
}

pub struct StatsManager {
    inner: Arc<Mutex<InnerStats>>,
    start_time: Instant,
}

impl StatsManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerStats {
                players: 0,
                playing_players: 0,
                start_time: Instant::now(),
                api_requests: HashMap::new(),
                api_errors: HashMap::new(),
                sources: HashMap::new(),
                playback_events: HashMap::new(),
                frames_sent: 0,
                frames_nulled: 0,
                frames_deficit: 0,
                frames_expected: 0,
            })),
            start_time: Instant::now(),
        }
    }

    pub async fn increment_api_request(&self, endpoint: &str) {
        let mut inner = self.inner.lock().await;
        *inner.api_requests.entry(endpoint.to_string()).or_insert(0) += 1;
    }

    pub async fn increment_api_error(&self, endpoint: &str) {
        let mut inner = self.inner.lock().await;
        *inner.api_errors.entry(endpoint.to_string()).or_insert(0) += 1;
    }

    pub async fn increment_source_success(&self, source: &str) {
        let mut inner = self.inner.lock().await;
        let entry = inner.sources.entry(source.to_string()).or_insert(SourceStats::default());
        entry.success += 1;
    }

    pub async fn increment_source_failure(&self, source: &str) {
        let mut inner = self.inner.lock().await;
        let entry = inner.sources.entry(source.to_string()).or_insert(SourceStats::default());
        entry.failure += 1;
    }

    pub async fn increment_playback_event(&self, event: &str) {
        let mut inner = self.inner.lock().await;
        *inner.playback_events.entry(event.to_string()).or_insert(0) += 1;
    }

    pub async fn set_players(&self, count: u32) {
        self.inner.lock().await.players = count;
    }

    pub async fn set_playing_players(&self, count: u32) {
        self.inner.lock().await.playing_players = count;
    }

    pub async fn add_frames(&self, sent: u64, nulled: u64, deficit: u64, expected: u64) {
        let mut inner = self.inner.lock().await;
        inner.frames_sent = inner.frames_sent.saturating_add(sent);
        inner.frames_nulled = inner.frames_nulled.saturating_add(nulled);
        inner.frames_deficit = inner.frames_deficit.saturating_add(deficit);
        inner.frames_expected = inner.frames_expected.saturating_add(expected);
    }

    pub async fn get_snapshot(&self) -> StatsSnapshot {
        let inner = self.inner.lock().await;
        let uptime = self.start_time.elapsed().as_millis() as u64;
        StatsSnapshot {
            players: inner.players,
            playing_players: inner.playing_players,
            uptime_ms: uptime,
            memory: MemoryStats::default(),
            cpu: CpuStats::default(),
            frame_stats: FrameStats {
                sent: inner.frames_sent,
                nulled: inner.frames_nulled,
                deficit: inner.frames_deficit,
                expected: inner.frames_expected,
            },
            api: ApiStats {
                requests: inner.api_requests.clone(),
                errors: inner.api_errors.clone(),
            },
            sources: inner.sources.clone(),
            playback: PlaybackStats {
                events: inner.playback_events.clone(),
            },
        }
    }
}

impl Default for StatsManager {
    fn default() -> Self {
        Self::new()
    }
}
