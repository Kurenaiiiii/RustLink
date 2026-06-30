use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Command types that can be sent to a playback worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlaybackWorkerCommand {
    CreatePlayer {
        guild_id: String,
        session_id: String,
        user_id: String,
    },
    DestroyPlayer {
        guild_id: String,
    },
    RestorePlayer {
        guild_id: String,
        state: serde_json::Value,
    },
    PlayerCommand {
        guild_id: String,
        command: serde_json::Value,
    },
    LoadTracks {
        query: String,
        source: Option<String>,
    },
    LoadLyrics {
        title: String,
        artist: String,
        identifier: Option<String>,
    },
    LoadMeaning {
        title: String,
        artist: String,
    },
    LoadChapters {
        track_id: String,
    },
    GetSources,
    GetTrackUrl {
        encoded_track: String,
    },
    LoadStream {
        track_id: String,
    },
    CancelStream {
        track_id: String,
    },
    UpdateYoutubeConfig {
        config: serde_json::Value,
    },
    ProfilerCommand {
        command: serde_json::Value,
    },
    Ping {
        timestamp: u64,
    },
    Shutdown,
}

/// Results from a playback worker command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerResult {
    Success(serde_json::Value),
    Error(String),
    Pong {
        timestamp: u64,
    },
}

/// Command types for source workers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SourceWorkerTask {
    Resolve {
        query: String,
        source: Option<String>,
    },
    Search {
        query: String,
        source: Option<String>,
    },
    UnifiedSearch {
        query: String,
    },
    LoadLyrics {
        title: String,
        artist: String,
        identifier: Option<String>,
    },
    LoadMeaning {
        title: String,
        artist: String,
    },
    LoadChapters {
        track_id: String,
    },
    LoadStream {
        encoded_track: String,
    },
    LoadLiveChat {
        channel_id: String,
    },
    CancelLiveChat {
        channel_id: String,
    },
    ProfilerCommand(serde_json::Value),
}

/// A command envelope sent from WorkerPool to a worker task.
pub struct WorkerCommandEnvelope {
    pub command: PlaybackWorkerCommand,
    pub response_tx: Option<oneshot::Sender<WorkerResult>>,
}

/// Stats reported by a worker task to the pool.
#[derive(Debug, Clone)]
pub struct WorkerTaskStats {
    pub worker_id: String,
    pub guild_count: u32,
    pub playing_count: u32,
    pub cpu_load: f64,
    pub command_queue_len: u32,
    pub frames_sent: u64,
    pub frames_nulled: u64,
    pub frames_deficit: u64,
    pub memory_used_bytes: u64,
    pub uptime_secs: u64,
}
