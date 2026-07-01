//! Source Worker — separate process spawned by SourceWorkerManager.
//!
//! Has two execution modes matching NodeLink's workers/source.ts:
//! - Main thread: spawns micro-worker pool, connects to source socket
//! - Worker thread: executes actual source API tasks (resolve, search, loadStream, etc.)
//!
//! Usage: source-worker --source-socket <path> [--worker-id <id>] [--micro-workers <n>]

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, warn};

use rustlink::workers::ipc_transport;

use rustlink::config::NodeLinkConfig;
use rustlink::managers::lyrics_manager::LyricsManager;
use rustlink::managers::meaning_manager::MeaningManager;
use rustlink::managers::source_manager::SourceManager;
use rustlink::providers::{GoogleTtsProvider, HttpProvider, LocalProvider};
use rustlink::sources::SourceRegistry;
use rustlink::sources::youtube::YouTubeSource;
use rustlink::workers::ipc::{IpcFrame, make_worker_socket_path};
use rustlink::workers::types::SourceWorkerTask;

const RECONNECT_DELAY_MS: u64 = 1000;
const FRAME_BUFFER_SIZE: usize = 65536;

struct SourceWorker {
    worker_id: String,
    source_socket_path: String,
    started_at: Instant,
    running: bool,
    source_manager: Arc<SourceManager>,
    lyrics_manager: Arc<LyricsManager>,
    meaning_manager: Arc<MeaningManager>,
}

impl SourceWorker {
    fn new(
        worker_id: String,
        source_socket_path: String,
        source_manager: Arc<SourceManager>,
        lyrics_manager: Arc<LyricsManager>,
        meaning_manager: Arc<MeaningManager>,
    ) -> Self {
        Self {
            worker_id,
            source_socket_path,
            started_at: Instant::now(),
            running: false,
            source_manager,
            lyrics_manager,
            meaning_manager,
        }
    }

    async fn connect_source_socket(&self) -> Result<ipc_transport::ClientStream, String> {
        ipc_transport::connect_client(&self.source_socket_path)
            .await
            .map_err(|e| format!("Failed to open source socket: {}", e))
    }

    async fn send_result(socket: &mut ipc_transport::ClientStream, id: &str, payload: &[u8]) -> Result<(), String> {
        let frame = IpcFrame::encode(0, id, payload);
        socket.write_all(&frame).await.map_err(|e| format!("Send error: {}", e))
    }

    async fn send_end(socket: &mut ipc_transport::ClientStream, id: &str) -> Result<(), String> {
        let frame = IpcFrame::encode(1, id, b"");
        socket.write_all(&frame).await.map_err(|e| format!("Send error: {}", e))
    }

    async fn send_error(socket: &mut ipc_transport::ClientStream, id: &str, msg: &str) -> Result<(), String> {
        let frame = IpcFrame::encode(2, id, msg.as_bytes());
        socket.write_all(&frame).await.map_err(|e| format!("Send error: {}", e))
    }

    async fn send_json_result<T: serde::Serialize>(
        socket: &mut ipc_transport::ClientStream,
        id: &str,
        value: &T,
    ) -> Result<(), String> {
        let data = serde_json::to_vec(value).map_err(|e| format!("Serialize error: {}", e))?;
        Self::send_result(socket, id, &data).await?;
        Self::send_end(socket, id).await
    }

    async fn send_json_error(socket: &mut ipc_transport::ClientStream, id: &str, msg: &str) -> Result<(), String> {
        Self::send_error(socket, id, msg).await
    }

    async fn handle_task(&self, id: &str, task: &SourceWorkerTask, socket: &mut ipc_transport::ClientStream) -> Result<(), String> {
        match task {
            SourceWorkerTask::Resolve { query, source: _ } => {
                match self.source_manager.resolve(query).await {
                    Ok(res) => Self::send_json_result(socket, id, &res).await,
                    Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                }
            }
            SourceWorkerTask::Search { query, source } => {
                let source_name = source.as_deref().unwrap_or("youtube");
                match self.source_manager.search(source_name, query).await {
                    Ok(res) => Self::send_json_result(socket, id, &res).await,
                    Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                }
            }
            SourceWorkerTask::UnifiedSearch { query } => {
                match self.source_manager.resolve(query).await {
                    Ok(res) => Self::send_json_result(socket, id, &res).await,
                    Err(_) => {
                        match self.source_manager.search_with_default("youtube", query).await {
                            Ok(res) => Self::send_json_result(socket, id, &res).await,
                            Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                        }
                    }
                }
            }
            SourceWorkerTask::LoadLyrics { title, artist, identifier } => {
                if identifier.is_some() {
                    match rustlink::lyrics::fetch_lyrics(title, artist, None, identifier.as_deref()).await {
                        Ok(Some(data)) => Self::send_json_result(socket, id, &data).await,
                        Ok(None) => {
                            match self.lyrics_manager.load_lyrics(title, artist, None, None, None).await {
                                Ok(res) => Self::send_json_result(socket, id, &res).await,
                                Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                            }
                        }
                        Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                    }
                } else {
                    match self.lyrics_manager.load_lyrics(title, artist, None, None, None).await {
                        Ok(res) => Self::send_json_result(socket, id, &res).await,
                        Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                    }
                }
            }
            SourceWorkerTask::LoadMeaning { title, artist } => {
                match self.meaning_manager.load_meaning(title, artist, "en", None).await {
                    Ok(Some(res)) => Self::send_json_result(socket, id, &res).await,
                    Ok(None) => {
                        let empty = serde_json::json!({"loadType": "empty"});
                        Self::send_json_result(socket, id, &empty).await
                    }
                    Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                }
            }
            SourceWorkerTask::LoadChapters { track_id } => {
                match rustlink::tracks::decode_track(track_id) {
                    Ok(track_data) => {
                        match self.source_manager.get_chapters(&track_data.info).await {
                            Ok(chapters) => Self::send_json_result(socket, id, &chapters).await,
                            Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                        }
                    }
                    Err(e) => Self::send_json_error(socket, id, &format!("Decode error: {}", e)).await,
                }
            }
            SourceWorkerTask::LoadStream { encoded_track } => {
                match rustlink::tracks::decode_track(encoded_track) {
                    Ok(track_data) => {
                        match self.source_manager.get_track_url(&track_data.info).await {
                            Ok(url_result) => Self::send_json_result(socket, id, &url_result).await,
                            Err(e) => Self::send_json_error(socket, id, &e.to_string()).await,
                        }
                    }
                    Err(e) => Self::send_json_error(socket, id, &format!("Decode error: {}", e)).await,
                }
            }
            SourceWorkerTask::LoadLiveChat { channel_id: _ } => {
                Self::send_json_error(socket, id, "LiveChat not yet implemented in source workers").await
            }
            SourceWorkerTask::CancelLiveChat { channel_id: _ } => {
                let ok = serde_json::json!({"status": "cancelled"});
                Self::send_json_result(socket, id, &ok).await
            }
            SourceWorkerTask::ProfilerCommand(_) => {
                let ok = serde_json::json!({"status": "ok", "workerType": "source"});
                Self::send_json_result(socket, id, &ok).await
            }
        }
    }

    async fn run_loop(&mut self) {
        self.running = true;

        while self.running {
            let mut socket = match self.connect_source_socket().await {
                Ok(c) => c,
                Err(e) => {
                    error!(target: "SourceWorker", "Socket connection failed: {}", e);
                    tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)).await;
                    continue;
                }
            };

            info!(target: "SourceWorker", "Worker {} connected", self.worker_id);

            let mut buffer = Vec::with_capacity(FRAME_BUFFER_SIZE);

            loop {
                match Self::read_one_task(&mut socket, &mut buffer).await {
                    Ok(Some((id, task))) => {
                        if matches!(&task, SourceWorkerTask::ProfilerCommand(v) if v.as_str() == Some("shutdown")) {
                            self.running = false;
                            break;
                        }
                        if let Err(e) = self.handle_task(&id, &task, &mut socket).await {
                            warn!(target: "SourceWorker", "Task handler error: {}", e);
                            break;
                        }
                    }
                    Ok(None) => {
                        info!(target: "SourceWorker", "Socket closed");
                        break;
                    }
                    Err(e) => {
                        warn!(target: "SourceWorker", "Read error: {}", e);
                        break;
                    }
                }
            }

            if self.running {
                info!(target: "SourceWorker", "Disconnected, reconnecting...");
                tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)).await;
            }
        }

        info!(target: "SourceWorker", "Worker {} shutting down", self.worker_id);
    }

    /// Read one task frame from the source socket.
    /// Returns (id, task) where task is deserialized from the JSON payload.
    async fn read_one_task<R: AsyncReadExt + Unpin>(
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<(String, SourceWorkerTask)>, String> {
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
        let _frame_type = header[1];
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
        let payload = &buffer[id_size..];

        let task: SourceWorkerTask = serde_json::from_slice(payload)
            .map_err(|e| format!("Failed to deserialize task: {}", e))?;

        Ok(Some((id, task)))
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut source_socket_path = String::new();
    let mut worker_id = String::new();
    let mut config_path = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" | "-s" => {
                i += 1;
                if i < args.len() {
                    source_socket_path = args[i].clone();
                }
            }
            "--id" | "-i" => {
                i += 1;
                if i < args.len() {
                    worker_id = args[i].clone();
                }
            }
            "--config" | "-c" => {
                i += 1;
                if i < args.len() {
                    config_path = args[i].clone();
                }
            }
            _ => {}
        }
        i += 1;
    }

    if source_socket_path.is_empty() {
        source_socket_path = make_worker_socket_path("source");
    }
    if worker_id.is_empty() {
        worker_id = format!("source-worker-{}", std::process::id());
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

    info!(target: "SourceWorker", "Starting worker_id={}, socket={}",
        worker_id, source_socket_path);

    let mut worker = SourceWorker::new(
        worker_id,
        source_socket_path,
        source_manager,
        lyrics_manager,
        meaning_manager,
    );
    worker.run_loop().await;
}