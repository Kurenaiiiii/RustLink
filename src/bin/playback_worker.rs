//! Playback Worker — separate process spawned by WorkerManager.
//!
//! Connects to the master via two Unix domain sockets (named pipes on Windows):
//! - Command socket (bidirectional): receives commands, sends results
//! - Event socket (unidirectional): sends events, stats, stream data
//!
//! Usage: playback_worker --event-socket <path> --command-socket <path> [--worker-id <id>]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, warn};

use rustlink::workers::ipc_transport;

use rustlink::config::NodeLinkConfig;
use rustlink::managers::source_manager::SourceManager;
use rustlink::managers::lyrics_manager::LyricsManager;
use rustlink::managers::meaning_manager::MeaningManager;
use rustlink::providers::{GoogleTtsProvider, HttpProvider, LocalProvider};
use rustlink::sources::SourceRegistry;
use rustlink::sources::youtube::YouTubeSource;
use rustlink::workers::ipc::{CommandFrameType, IpcFrame, IpcHello, make_worker_socket_path};
use rustlink::workers::types::PlaybackWorkerCommand;

const FRAME_BUFFER_SIZE: usize = 65536;
const STATS_INTERVAL_SECS: u64 = 5;
const RECONNECT_DELAY_MS: u64 = 1000;

struct PlaybackWorker {
    worker_id: String,
    event_socket_path: String,
    command_socket_path: String,
    started_at: Instant,
    running: bool,
    source_manager: Arc<SourceManager>,
    lyrics_manager: Arc<LyricsManager>,
    meaning_manager: Arc<MeaningManager>,
    players: HashMap<String, PlayerHandle>,
    total_commands: u64,
    frames_sent: u64,
    frames_nulled: u64,
    frames_deficit: u64,
}

struct PlayerHandle {
    guild_id: String,
    created_at: Instant,
}

impl PlaybackWorker {
    fn new(
        worker_id: String,
        event_path: String,
        command_path: String,
        source_manager: Arc<SourceManager>,
        lyrics_manager: Arc<LyricsManager>,
        meaning_manager: Arc<MeaningManager>,
    ) -> Self {
        Self {
            worker_id,
            event_socket_path: event_path,
            command_socket_path: command_path,
            started_at: Instant::now(),
            running: false,
            source_manager,
            lyrics_manager,
            meaning_manager,
            players: HashMap::new(),
            total_commands: 0,
            frames_sent: 0,
            frames_nulled: 0,
            frames_deficit: 0,
        }
    }

    async fn connect_event_socket(&self) -> Result<ipc_transport::ClientStream, String> {
        ipc_transport::connect_client(&self.event_socket_path)
            .await
            .map_err(|e| format!("Failed to open event socket: {}", e))
    }

    async fn connect_command_socket(&self) -> Result<ipc_transport::ClientStream, String> {
        ipc_transport::connect_client(&self.command_socket_path)
            .await
            .map_err(|e| format!("Failed to open command socket: {}", e))
    }

    async fn send_hello(command_client: &mut ipc_transport::ClientStream, pid: u32, worker_id: &str) -> Result<(), String> {
        let hello = IpcHello {
            pid,
            worker_type: format!("playback-{}", worker_id),
        };
        let frame = IpcFrame::encode_json(CommandFrameType::Hello as u8, "", &hello);
        command_client
            .write_all(&frame)
            .await
            .map_err(|e| format!("Failed to send hello: {}", e))
    }

    async fn send_stats(event_client: &mut ipc_transport::ClientStream, stats: &serde_json::Value) -> Result<(), String> {
        let frame = IpcFrame::encode_json(CommandFrameType::Result as u8, "stats", stats);
        event_client
            .write_all(&frame)
            .await
            .map_err(|e| format!("Failed to send stats: {}", e))
    }

    async fn send_result(command_client: &mut ipc_transport::ClientStream, id: &str, payload: &[u8]) -> Result<(), String> {
        let frame = IpcFrame::encode(CommandFrameType::Result as u8, id, payload);
        command_client.write_all(&frame).await.map_err(|e| format!("Send error: {}", e))
    }

    async fn send_json_result<T: serde::Serialize>(
        command_client: &mut ipc_transport::ClientStream,
        id: &str,
        value: &T,
    ) -> Result<(), String> {
        let data = serde_json::to_vec(value).map_err(|e| format!("Serialize error: {}", e))?;
        Self::send_result(command_client, id, &data).await
    }

    async fn send_error(command_client: &mut ipc_transport::ClientStream, id: &str, msg: &str) -> Result<(), String> {
        let frame = IpcFrame::encode(CommandFrameType::Error as u8, id, msg.as_bytes());
        command_client.write_all(&frame).await.map_err(|e| format!("Send error: {}", e))
    }

    async fn handle_command(
        &mut self,
        id: &str,
        command: PlaybackWorkerCommand,
        command_client: &mut ipc_transport::ClientStream,
    ) -> Result<(), String> {
        self.total_commands += 1;
        match command {
            PlaybackWorkerCommand::CreatePlayer { guild_id, session_id: _, user_id: _ } => {
                if self.players.contains_key(&guild_id) {
                    Self::send_json_result(command_client, id, &serde_json::json!({
                        "status": "already_exists", "guildId": guild_id
                    })).await
                } else {
                    self.players.insert(guild_id.clone(), PlayerHandle {
                        guild_id: guild_id.clone(),
                        created_at: Instant::now(),
                    });
                    info!(target: "PlaybackWorker", "Player created for guild {} on worker {}", guild_id, self.worker_id);
                    Self::send_json_result(command_client, id, &serde_json::json!({
                        "status": "created", "guildId": guild_id, "workerId": self.worker_id
                    })).await
                }
            }
            PlaybackWorkerCommand::DestroyPlayer { guild_id } => {
                self.players.remove(&guild_id);
                info!(target: "PlaybackWorker", "Player destroyed for guild {}", guild_id);
                Self::send_json_result(command_client, id, &serde_json::json!({
                    "status": "destroyed", "guildId": guild_id
                })).await
            }
            PlaybackWorkerCommand::RestorePlayer { guild_id, state: _ } => {
                self.players.insert(guild_id.clone(), PlayerHandle {
                    guild_id: guild_id.clone(),
                    created_at: Instant::now(),
                });
                Self::send_json_result(command_client, id, &serde_json::json!({
                    "status": "restored", "guildId": guild_id
                })).await
            }
            PlaybackWorkerCommand::PlayerCommand { guild_id, command: _ } => {
                if self.players.contains_key(&guild_id) {
                    Self::send_json_result(command_client, id, &serde_json::json!({
                        "status": "command_sent", "guildId": guild_id
                    })).await
                } else {
                    Self::send_error(command_client, id, &format!("No player for guild {}", guild_id)).await
                }
            }
            PlaybackWorkerCommand::LoadTracks { query, source } => {
                match source {
                    Some(src) if !src.is_empty() && src != "default" => {
                        match self.source_manager.search(&src, &query).await {
                            Ok(res) => Self::send_json_result(command_client, id, &res).await,
                            Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                        }
                    }
                    _ => {
                        match self.source_manager.resolve(&query).await {
                            Ok(res) => Self::send_json_result(command_client, id, &res).await,
                            Err(_) => {
                                match self.source_manager.search_with_default("youtube", &query).await {
                                    Ok(res) => Self::send_json_result(command_client, id, &res).await,
                                    Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                                }
                            }
                        }
                    }
                }
            }
            PlaybackWorkerCommand::LoadLyrics { title, artist, identifier } => {
                if identifier.is_some() {
                    match rustlink::lyrics::fetch_lyrics(&title, &artist, None, identifier.as_deref()).await {
                        Ok(Some(data)) => Self::send_json_result(command_client, id, &data).await,
                        Ok(None) => {
                            match self.lyrics_manager.load_lyrics(&title, &artist, None, None, None).await {
                                Ok(res) => Self::send_json_result(command_client, id, &res).await,
                                Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                            }
                        }
                        Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                    }
                } else {
                    match self.lyrics_manager.load_lyrics(&title, &artist, None, None, None).await {
                        Ok(res) => Self::send_json_result(command_client, id, &res).await,
                        Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                    }
                }
            }
            PlaybackWorkerCommand::LoadMeaning { title, artist } => {
                match self.meaning_manager.load_meaning(&title, &artist, "en", None).await {
                    Ok(Some(res)) => Self::send_json_result(command_client, id, &res).await,
                    Ok(None) => {
                        let empty = serde_json::json!({"loadType": "empty"});
                        Self::send_json_result(command_client, id, &empty).await
                    }
                    Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                }
            }
            PlaybackWorkerCommand::LoadChapters { track_id } => {
                match rustlink::tracks::decode_track(&track_id) {
                    Ok(track_data) => {
                        match self.source_manager.get_chapters(&track_data.info).await {
                            Ok(chapters) => Self::send_json_result(command_client, id, &chapters).await,
                            Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                        }
                    }
                    Err(e) => Self::send_error(command_client, id, &format!("Decode error: {}", e)).await,
                }
            }
            PlaybackWorkerCommand::GetSources => {
                let names = self.source_manager.source_names().await;
                Self::send_json_result(command_client, id, &serde_json::json!({"sources": names})).await
            }
            PlaybackWorkerCommand::GetTrackUrl { encoded_track } => {
                match rustlink::tracks::decode_track(&encoded_track) {
                    Ok(track_data) => {
                        match self.source_manager.get_track_url(&track_data.info).await {
                            Ok(url_result) => Self::send_json_result(command_client, id, &url_result).await,
                            Err(e) => Self::send_error(command_client, id, &e.to_string()).await,
                        }
                    }
                    Err(e) => Self::send_error(command_client, id, &format!("Decode error: {}", e)).await,
                }
            }
            PlaybackWorkerCommand::LoadStream { track_id: _ } => {
                Self::send_error(command_client, id, "loadStream not yet implemented in process workers").await
            }
            PlaybackWorkerCommand::CancelStream { track_id: _ } => {
                Self::send_json_result(command_client, id, &serde_json::json!({"status": "cancelled"})).await
            }
            PlaybackWorkerCommand::UpdateYoutubeConfig { config: _ } => {
                Self::send_json_result(command_client, id, &serde_json::json!({"status": "updated"})).await
            }
            PlaybackWorkerCommand::ProfilerCommand { command: _ } => {
                Self::send_json_result(command_client, id, &serde_json::json!({
                    "status": "ok", "workerType": "playback"
                })).await
            }
            PlaybackWorkerCommand::Ping { timestamp } => {
                Self::send_json_result(command_client, id, &serde_json::json!({
                    "type": "pong", "timestamp": timestamp
                })).await
            }
            PlaybackWorkerCommand::Shutdown => {
                self.running = false;
                self.players.clear();
                Self::send_json_result(command_client, id, &serde_json::json!({"shutdown": true})).await
            }
        }
    }

    async fn handle_incoming_frame(
        &mut self,
        frame_type: u8,
        id: String,
        payload: Vec<u8>,
        command_client: &mut ipc_transport::ClientStream,
    ) -> Result<(), String> {
        match CommandFrameType::from_u8(frame_type) {
            Some(CommandFrameType::Command) => {
                // Incoming command from master — parse and handle it
                match serde_json::from_slice::<PlaybackWorkerCommand>(&payload) {
                    Ok(cmd) => self.handle_command(&id, cmd, command_client).await,
                    Err(e) => {
                        warn!(target: "PlaybackWorker", "Failed to parse command (id={}): {}", id, e);
                        Self::send_error(command_client, &id, &format!("Parse error: {}", e)).await
                    }
                }
            }
            Some(CommandFrameType::Ping) => {
                // Master is pinging us — respond with PONG
                let ts = serde_json::from_slice::<serde_json::Value>(&payload)
                    .ok()
                    .and_then(|v| v["timestamp"].as_u64())
                    .unwrap_or(0);
                let pong = serde_json::json!({"type": "pong", "timestamp": ts});
                Self::send_json_result(command_client, &id, &pong).await
            }
            Some(CommandFrameType::RotateSocket) => {
                // Master requesting socket rotation — acknowledge, will reconnect
                let ack = serde_json::json!({"status": "ok"});
                Self::send_json_result(command_client, &id, &ack).await
            }
            Some(CommandFrameType::Hello) => {
                // Master sent us a hello (unexpected but harmless)
                info!(target: "PlaybackWorker", "Received hello from master (id={})", id);
                Ok(())
            }
            Some(CommandFrameType::Result) | Some(CommandFrameType::Error) => {
                // We shouldn't receive result/error from the master
                warn!(target: "PlaybackWorker", "Unexpected {} frame from master (id={})",
                    if frame_type == 2 { "result" } else { "error" }, id);
                Ok(())
            }
            Some(CommandFrameType::Pong) => {
                // Pong from master — can ignore, our pings are one-way
                Ok(())
            }
            None => {
                warn!(target: "PlaybackWorker", "Unknown frame type {} from master (id={})", frame_type, id);
                Ok(())
            }
        }
    }

    async fn run_loop(&mut self) {
        self.running = true;
        let pid = std::process::id();

        while self.running {
            let mut event_client = match self.connect_event_socket().await {
                Ok(c) => c,
                Err(e) => {
                    error!(target: "PlaybackWorker", "Event socket connection failed: {}", e);
                    tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)).await;
                    continue;
                }
            };
            let mut command_client = match self.connect_command_socket().await {
                Ok(c) => c,
                Err(e) => {
                    error!(target: "PlaybackWorker", "Command socket connection failed: {}", e);
                    tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)).await;
                    continue;
                }
            };

            // Send HELLO
            if let Err(e) = Self::send_hello(&mut command_client, pid, &self.worker_id).await {
                error!(target: "PlaybackWorker", "Failed to send hello: {}", e);
                tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)).await;
                continue;
            }
            info!(target: "PlaybackWorker", "Worker {} registered with master (pid={})", self.worker_id, pid);

            let mut buffer = Vec::with_capacity(FRAME_BUFFER_SIZE);
            let mut last_stats_send = Instant::now();

            loop {
                // Read next frame with timeout (to allow periodic stats)
                let frame_result = tokio::time::timeout(
                    Duration::from_secs(1),
                    Self::read_one_frame(&mut command_client, &mut buffer),
                ).await;

                match frame_result {
                    Ok(Ok(Some((frame_type, id, payload)))) => {
                        if let Err(e) = self.handle_incoming_frame(frame_type, id, payload, &mut command_client).await {
                            warn!(target: "PlaybackWorker", "Frame handler error: {}", e);
                            break;
                        }
                    }
                    Ok(Ok(None)) => {
                        info!(target: "PlaybackWorker", "Command stream ended");
                        break;
                    }
                    Ok(Err(e)) => {
                        warn!(target: "PlaybackWorker", "Frame read error: {}", e);
                        break;
                    }
                    Err(_timeout) => {
                        // Timeout is normal — send periodic stats if due
                        if last_stats_send.elapsed() >= Duration::from_secs(STATS_INTERVAL_SECS) {
                            let uptime = self.started_at.elapsed().as_secs();
                            let stats = serde_json::json!({
                                "type": "stats",
                                "workerId": self.worker_id,
                                "players": self.players.len(),
                                "playingPlayers": 0,
                                "uptime": uptime,
                                "framesSent": self.frames_sent,
                                "framesNulled": self.frames_nulled,
                                "framesDeficit": self.frames_deficit,
                                "totalCommands": self.total_commands,
                            });
                            let _ = Self::send_stats(&mut event_client, &stats).await;
                            last_stats_send = Instant::now();
                        }

                        if !self.running {
                            break;
                        }
                    }
                }
            }

            if self.running {
                info!(target: "PlaybackWorker", "Disconnected, reconnecting in {}ms...", RECONNECT_DELAY_MS);
                tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)).await;
            }
        }

        info!(target: "PlaybackWorker", "Worker {} shutting down", self.worker_id);
    }

    /// Read one complete frame from a socket.
    async fn read_one_frame<R: AsyncReadExt + Unpin>(
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<(u8, String, Vec<u8>)>, String> {
        let mut header = [0u8; 6];
        let mut read = 0;
        while read < 6 {
            let n = reader
                .read(&mut header[read..])
                .await
                .map_err(|e| format!("Read error: {}", e))?;
            if n == 0 {
                return if read == 0 { Ok(None) } else { Err("Incomplete header".to_string()) };
            }
            read += n;
        }

        let id_size = header[0] as usize;
        let frame_type = header[1];
        let payload_size = u32::from_be_bytes([header[2], header[3], header[4], header[5]]) as usize;
        let total_body = id_size + payload_size;

        buffer.clear();
        buffer.resize(total_body, 0);
        read = 0;
        while read < total_body {
            let n = reader
                .read(&mut buffer[read..])
                .await
                .map_err(|e| format!("Read error: {}", e))?;
            if n == 0 {
                return Err("Incomplete frame body".to_string());
            }
            read += n;
        }

        let id = String::from_utf8_lossy(&buffer[..id_size]).to_string();
        let payload = buffer[id_size..].to_vec();
        Ok(Some((frame_type, id, payload)))
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut event_socket_path = String::new();
    let mut command_socket_path = String::new();
    let mut worker_id = String::new();
    let mut config_path = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--event" | "-e" => {
                i += 1;
                if i < args.len() {
                    event_socket_path = args[i].clone();
                }
            }
            "--command" | "-c" => {
                i += 1;
                if i < args.len() {
                    command_socket_path = args[i].clone();
                }
            }
            "--id" | "-i" => {
                i += 1;
                if i < args.len() {
                    worker_id = args[i].clone();
                }
            }
            "--config" | "-f" => {
                i += 1;
                if i < args.len() {
                    config_path = args[i].clone();
                }
            }
            _ => {}
        }
        i += 1;
    }

    if event_socket_path.is_empty() {
        event_socket_path = make_worker_socket_path("event");
    }
    if command_socket_path.is_empty() {
        command_socket_path = make_worker_socket_path("command");
    }
    if worker_id.is_empty() {
        worker_id = format!("worker-{}", std::process::id());
    }
    if config_path.is_empty() {
        config_path = "rustlink.toml".to_string();
    }

    let config = NodeLinkConfig::load_or_default(&config_path).unwrap_or_default();
    let level = &config.logging.level;
    tracing_subscriber::fmt()
        .with_target(true)
        .with_line_number(true)
        .with_max_level(match *level {
            "trace" => tracing::Level::TRACE,
            "debug" => tracing::Level::DEBUG,
            "warn" => tracing::Level::WARN,
            "error" => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        })
        .init();

    // Initialize sources
    let sources = SourceRegistry::default();
    if config.sources.youtube.enabled {
        let yt = YouTubeSource::new(
            config.sources.youtube.hl.clone(),
            config.sources.youtube.gl.clone(),
            config.sources.youtube.allow_itag.clone(),
            config.sources.youtube.refresh_tokens.clone(),
            config.sources.youtube.potoken.clone(),
            config.sources.youtube.po_token_endpoint.clone(),
        );
        yt.start_background_tasks();
        sources.register(yt).await;
    }
    if config.sources.http.enabled {
        sources.register(HttpProvider::default()).await;
    }
    if config.sources.local.enabled {
        sources.register(LocalProvider::new(config.sources.local.base_path.clone())).await;
    }
    if config.sources.google_tts.enabled {
        sources.register(GoogleTtsProvider::new(config.sources.google_tts.language.clone())).await;
    }

    let source_manager = Arc::new(SourceManager::new(sources));
    let lyrics_manager = Arc::new(LyricsManager::new(config.lyrics.clone()));
    let meaning_manager = {
        let mut mm = MeaningManager::new();
        mm.load_from_config(&config.meanings);
        Arc::new(mm)
    };

    info!(target: "PlaybackWorker", "Starting worker_id={}, event_socket={}, command_socket={}",
        worker_id, event_socket_path, command_socket_path);

    let mut worker = PlaybackWorker::new(
        worker_id,
        event_socket_path,
        command_socket_path,
        source_manager,
        lyrics_manager,
        meaning_manager,
    );
    worker.run_loop().await;
}