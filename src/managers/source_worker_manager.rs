use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, oneshot, mpsc};

use crate::workers::ipc_transport;
use tokio::time::interval;
use tracing::{info, warn};

use crate::config::{SpecializedSourceWorkerConfig, WorkerProcessMode};
use crate::managers::lyrics_manager::LyricsManager;
use crate::managers::meaning_manager::MeaningManager;
use crate::managers::source_manager::SourceManager;
use crate::workers::ipc::{IpcFrame, CommandFrameType, make_worker_socket_path};
use crate::workers::types::{SourceWorkerTask, WorkerResult};

#[derive(Debug, Clone)]
pub struct SourceWorkerTaskInfo {
    pub id: String,
    pub task_type: String,
    pub payload: serde_json::Value,
}

pub type SourceTaskResult = Result<serde_json::Value, String>;

struct SourceWorkerHandle {
    task_tx: mpsc::Sender<SourceTaskEnvelope>,
    join: Option<tokio::task::JoinHandle<()>>,
}

struct SourceTaskEnvelope {
    task: SourceWorkerTask,
    response_tx: oneshot::Sender<WorkerResult>,
}

struct SourceWorkerInfo {
    id: String,
    pid: u32,
    healthy: bool,
    pending_count: u32,
    total_tasks: u64,
    last_heartbeat: Instant,
    started_at: Instant,
}

struct PendingTask {
    task_type: String,
    worker_id: String,
    created_at: Instant,
}

pub struct SourceWorkerPool {
    workers: Arc<Mutex<HashMap<String, SourceWorkerInfo>>>,
    handles: Arc<Mutex<HashMap<String, SourceWorkerHandle>>>,
    pending_tasks: Arc<Mutex<HashMap<String, PendingTask>>>,
    config: SpecializedSourceWorkerConfig,
    process_mode: WorkerProcessMode,
    source_manager: Arc<SourceManager>,
    lyrics_manager: Arc<LyricsManager>,
    meaning_manager: Arc<MeaningManager>,
    stopped: Arc<AtomicBool>,
    next_worker_id: Arc<AtomicU32>,
}

impl SourceWorkerPool {
    pub fn new(
        config: SpecializedSourceWorkerConfig,
        process_mode: WorkerProcessMode,
        source_manager: Arc<SourceManager>,
        lyrics_manager: Arc<LyricsManager>,
        meaning_manager: Arc<MeaningManager>,
    ) -> Self {
        Self {
            workers: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
            pending_tasks: Arc::new(Mutex::new(HashMap::new())),
            config,
            process_mode,
            source_manager,
            lyrics_manager,
            meaning_manager,
            stopped: Arc::new(AtomicBool::new(false)),
            next_worker_id: Arc::new(AtomicU32::new(1)),
        }
    }

    pub async fn start(&self) {
        self.spawn_initial_workers().await;
        self.start_health_check();
        info!(target: "SourceWorkerPool", "Source worker pool initialized. {} workers",
            self.config.micro_workers);
    }

    async fn spawn_initial_workers(&self) {
        for _ in 0..self.config.micro_workers.max(1) {
            self.spawn_worker().await;
        }
    }

    async fn spawn_worker(&self) -> String {
        match self.process_mode {
            WorkerProcessMode::MultiProcess => self.spawn_worker_process().await,
            WorkerProcessMode::InProcess => self.spawn_worker_task().await,
        }
    }

    async fn spawn_worker_task(&self) -> String {
        let id_num = self.next_worker_id.fetch_add(1, Ordering::SeqCst);
        let id = format!("source-worker-{}", id_num);

        let (task_tx, mut task_rx) = mpsc::channel::<SourceTaskEnvelope>(64);

        let source_manager = self.source_manager.clone();
        let lyrics_manager = self.lyrics_manager.clone();
        let meaning_manager = self.meaning_manager.clone();

        let join = tokio::spawn(async move {
            while let Some(envelope) = task_rx.recv().await {
                let result = match envelope.task {
                    SourceWorkerTask::Resolve { query, source: _ } => {
                        match source_manager.resolve(&query).await {
                            Ok(res) => serialize_result(&res),
                            Err(e) => WorkerResult::Error(e.to_string()),
                        }
                    }
                    SourceWorkerTask::Search { query, source } => {
                        let source_name = source.as_deref().unwrap_or("youtube");
                        match source_manager.search(source_name, &query).await {
                            Ok(res) => serialize_result(&res),
                            Err(e) => WorkerResult::Error(e.to_string()),
                        }
                    }
                    SourceWorkerTask::UnifiedSearch { query } => {
                        match source_manager.resolve(&query).await {
                            Ok(res) => serialize_result(&res),
                            Err(_) => {
                                match source_manager.search_with_default("youtube", &query).await {
                                    Ok(res) => serialize_result(&res),
                                    Err(e) => WorkerResult::Error(e.to_string()),
                                }
                            }
                        }
                    }
                    SourceWorkerTask::LoadLyrics { title, artist, identifier } => {
                        if identifier.is_some() {
                            match crate::lyrics::fetch_lyrics(&title, &artist, None, identifier.as_deref()).await {
                                Ok(Some(data)) => serialize_result(&data),
                                Ok(None) => {
                                    match lyrics_manager.load_lyrics(&title, &artist, None, None, None).await {
                                        Ok(res) => serialize_result(&res),
                                        Err(e) => WorkerResult::Error(e.to_string()),
                                    }
                                }
                                Err(e) => WorkerResult::Error(e.to_string()),
                            }
                        } else {
                            match lyrics_manager.load_lyrics(&title, &artist, None, None, None).await {
                                Ok(res) => serialize_result(&res),
                                Err(e) => WorkerResult::Error(e.to_string()),
                            }
                        }
                    }
                    SourceWorkerTask::LoadMeaning { title, artist } => {
                        match meaning_manager.load_meaning(&title, &artist, "en", None).await {
                            Ok(Some(res)) => serialize_result(&res),
                            Ok(None) => WorkerResult::Success(serde_json::json!({"loadType": "empty"})),
                            Err(e) => WorkerResult::Error(e.to_string()),
                        }
                    }
                    SourceWorkerTask::LoadChapters { track_id } => {
                        match crate::tracks::decode_track(&track_id) {
                            Ok(track_data) => {
                                match source_manager.get_chapters(&track_data.info).await {
                                    Ok(chapters) => serialize_result(&chapters),
                                    Err(e) => WorkerResult::Error(e.to_string()),
                                }
                            }
                            Err(e) => WorkerResult::Error(format!("Decode error: {}", e)),
                        }
                    }
                    SourceWorkerTask::LoadStream { encoded_track } => {
                        match crate::tracks::decode_track(&encoded_track) {
                            Ok(track_data) => {
                                match source_manager.get_track_url(&track_data.info).await {
                                    Ok(url_result) => serialize_result(&url_result),
                                    Err(e) => WorkerResult::Error(e.to_string()),
                                }
                            }
                            Err(e) => WorkerResult::Error(format!("Decode error: {}", e)),
                        }
                    }
                    SourceWorkerTask::LoadLiveChat { channel_id: _ } => {
                        WorkerResult::Error("LiveChat not yet implemented in source workers".to_string())
                    }
                    SourceWorkerTask::CancelLiveChat { channel_id: _ } => {
                        WorkerResult::Success(serde_json::json!({"status": "cancelled"}))
                    }
                    SourceWorkerTask::ProfilerCommand(_) => {
                        WorkerResult::Success(serde_json::json!({
                            "status": "ok",
                            "workerType": "source"
                        }))
                    }
                };
                let _ = envelope.response_tx.send(result);
            }
        });

        let worker = SourceWorkerInfo {
            id: id.clone(),
            pid: std::process::id(),
            healthy: true,
            pending_count: 0,
            total_tasks: 0,
            last_heartbeat: Instant::now(),
            started_at: Instant::now(),
        };

        self.workers.lock().await.insert(id.clone(), worker);
        self.handles.lock().await.insert(id.clone(), SourceWorkerHandle {
            task_tx,
            join: Some(join),
        });

        info!(target: "SourceWorkerPool", "Spawned source worker {}", id_num);
        id
    }

    async fn spawn_worker_process(&self) -> String {
        let id_num = self.next_worker_id.fetch_add(1, Ordering::SeqCst);
        let id = format!("source-worker-{}", id_num);

        let source_socket_path = make_worker_socket_path("source");

        let sock_path = source_socket_path.clone();

        // Start source socket server (waits for client connection on Unix)
        let source_server: tokio::task::JoinHandle<Option<ipc_transport::ServerStream>> = tokio::spawn(async move {
            match ipc_transport::create_server(&sock_path).await {
                Ok(stream) => {
                    info!(target: "SourceWorkerPool", "Source socket connected: {}", sock_path);
                    Some(stream)
                }
                Err(e) => {
                    warn!(target: "SourceWorkerPool", "Failed to create source socket: {}", e);
                    None
                }
            }
        });

        // Spawn the source-worker binary
        let exe_name = format!("source-worker{}", std::env::consts::EXE_SUFFIX);
        let exe_path = std::env::current_exe()
            .map(|p| p.parent().unwrap().join(&exe_name))
            .unwrap_or_else(|_| std::path::PathBuf::from(&exe_name));

        let mut child = match std::process::Command::new(&exe_path)
            .arg("--source-socket")
            .arg(&source_socket_path)
            .arg("--worker-id")
            .arg(&id)
            .arg("--micro-workers")
            .arg(self.config.micro_workers.to_string())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(target: "SourceWorkerPool", "Failed to spawn source-worker process: {}. Falling back to in-process.", e);
                return self.spawn_worker_task().await;
            }
        };

        let pid = child.id();
        let worker_info = SourceWorkerInfo {
            id: id.clone(),
            pid,
            healthy: true,
            pending_count: 0,
            total_tasks: 0,
            last_heartbeat: Instant::now(),
            started_at: Instant::now(),
        };

        // Accept connection with timeout
        let client = tokio::time::timeout(Duration::from_secs(10), async {
            source_server.await.ok().and_then(|r| r)
        }).await.unwrap_or(None);

        self.workers.lock().await.insert(id.clone(), worker_info);

        if let Some(socket_client) = client {
            // Create channel for task dispatch
            let (task_tx, task_rx) = mpsc::channel::<SourceTaskEnvelope>(64);

            let worker_id = id.clone();
            let workers_ref = self.workers.clone();
            let join = tokio::spawn(async move {
                handle_source_process_worker(
                    worker_id,
                    socket_client,
                    task_rx,
                    workers_ref,
                ).await;
            });

            self.handles.lock().await.insert(id.clone(), SourceWorkerHandle {
                task_tx,
                join: Some(join),
            });
            info!(target: "SourceWorkerPool", "Spawned process source worker {} (pid: {})", id_num, pid);
        } else {
            warn!(target: "SourceWorkerPool", "Source worker {} failed to connect socket, killing process", id);
            let _ = child.kill();
            self.workers.lock().await.remove(&id);
        }

        id
    }

    pub async fn delegate(
        &self,
        task_type: &str,
        _payload: serde_json::Value,
    ) -> Result<String, String> {
        let worker_id = self.find_best_worker().await?;
        let task_id = uuid::Uuid::new_v4().to_string();

        {
            let mut pending = self.pending_tasks.lock().await;
            pending.insert(task_id.clone(), PendingTask {
                task_type: task_type.to_string(),
                worker_id: worker_id.clone(),
                created_at: Instant::now(),
            });
        }

        {
            let mut workers = self.workers.lock().await;
            if let Some(info) = workers.get_mut(&worker_id) {
                info.pending_count += 1;
                info.total_tasks += 1;
            }
        }

        Ok(task_id)
    }

    /// Execute a task on a worker and wait for the result.
    pub async fn execute(
        &self,
        task: SourceWorkerTask,
    ) -> Result<WorkerResult, String> {
        let worker_id = self.find_best_worker().await?;

        let (tx, rx) = oneshot::channel();
        let envelope = SourceTaskEnvelope {
            task,
            response_tx: tx,
        };

        let handles = self.handles.lock().await;
        let handle = handles
            .get(&worker_id)
            .ok_or_else(|| format!("Source worker {} not found", worker_id))?;

        handle
            .task_tx
            .send(envelope)
            .await
            .map_err(|_| format!("Failed to send task to source worker {}", worker_id))?;
        drop(handles);

        {
            let mut workers = self.workers.lock().await;
            if let Some(info) = workers.get_mut(&worker_id) {
                info.pending_count += 1;
                info.total_tasks += 1;
            }
        }

        let result = rx
            .await
            .map_err(|_| format!("Source worker {} response channel closed", worker_id))?;

        {
            let mut workers = self.workers.lock().await;
            if let Some(info) = workers.get_mut(&worker_id) {
                info.pending_count = info.pending_count.saturating_sub(1);
            }
        }

        Ok(result)
    }

    pub async fn complete_task(&self, task_id: &str) {
        let mut pending = self.pending_tasks.lock().await;
        if let Some(task) = pending.remove(task_id) {
            let mut workers = self.workers.lock().await;
            if let Some(info) = workers.get_mut(&task.worker_id) {
                info.pending_count = info.pending_count.saturating_sub(1);
            }
        }
    }

    pub async fn worker_count(&self) -> usize {
        self.workers.lock().await.len()
    }

    pub async fn healthy_worker_count(&self) -> usize {
        self.workers
            .lock()
            .await
            .values()
            .filter(|w| w.healthy)
            .count()
    }

    pub async fn pending_task_count(&self) -> usize {
        self.pending_tasks.lock().await.len()
    }

    async fn find_best_worker(&self) -> Result<String, String> {
        let workers = self.workers.lock().await;
        let mut best: Option<(String, u32)> = None;

        for (id, info) in workers.iter() {
            if !info.healthy {
                continue;
            }
            match best {
                Some((_, best_load)) if info.pending_count < best_load => {
                    best = Some((id.clone(), info.pending_count));
                }
                None => {
                    best = Some((id.clone(), info.pending_count));
                }
                _ => {}
            }
        }

        if let Some((id, _)) = best {
            return Ok(id);
        }

        drop(workers);
        if self.worker_count().await < self.config.micro_workers.max(1) * 2 {
            return Ok(self.spawn_worker().await);
        }

        Err("No healthy source workers available".into())
    }

    fn start_health_check(&self) {
        let workers = self.workers.clone();
        let stopped = self.stopped.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(15));
            loop {
                ticker.tick().await;
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let mut w = workers.lock().await;
                let now = Instant::now();
                w.retain(|id, info| {
                    if !info.healthy && now.duration_since(info.last_heartbeat) > Duration::from_secs(60) {
                        warn!(target: "SourceWorkerPool", "Removing unhealthy source worker {}", id);
                        return false;
                    }
                    true
                });
            }
        });
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);

        let mut handles = self.handles.lock().await;
        for (id, mut handle) in handles.drain() {
            if let Some(join) = handle.join.take() {
                join.abort();
            }
            info!(target: "SourceWorkerPool", "Shut down source worker {}", id);
        }

        self.workers.lock().await.clear();
        self.pending_tasks.lock().await.clear();
        info!(target: "SourceWorkerPool", "Source worker pool shut down");
    }
}

/// Serialize a value to JSON WorkerResult.
fn serialize_result<T: serde::Serialize>(value: &T) -> WorkerResult {
    match serde_json::to_value(value) {
        Ok(v) => WorkerResult::Success(v),
        Err(e) => WorkerResult::Error(format!("Serialize error: {}", e)),
    }
}

/// Handle communication with a process-based source worker.
///
/// Reads tasks from task_rx, sends them over the named pipe socket,
/// reads back Data/End/Error frames, and forwards results via oneshot senders.
async fn handle_source_process_worker(
    worker_id: String,
    mut socket: ipc_transport::ServerStream,
    mut task_rx: mpsc::Receiver<SourceTaskEnvelope>,
    workers: Arc<Mutex<HashMap<String, SourceWorkerInfo>>>,
) {
    info!(target: "SourceWorkerPool", "Source process worker {} connected", worker_id);

    let mut pending: HashMap<String, oneshot::Sender<WorkerResult>> = HashMap::new();
    let mut msg_counter: u64 = 0;
    let mut read_buffer = Vec::with_capacity(65536);

    loop {
        tokio::select! {
            recv_result = read_one_source_frame(&mut socket, &mut read_buffer) => {
                match recv_result {
                    Ok(Some((frame_type, id, payload))) => {
                        match frame_type {
                            0 => { // Data frame
                                let result = serde_json::from_slice(&payload)
                                    .map(WorkerResult::Success)
                                    .unwrap_or_else(|_| {
                                        WorkerResult::Success(serde_json::Value::Null)
                                    });
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(result);
                                }
                            }
                            1 => { // End frame — no more data
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(WorkerResult::Success(serde_json::json!({"end": true})));
                                }
                            }
                            2 => { // Error frame
                                let msg = String::from_utf8_lossy(&payload).to_string();
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(WorkerResult::Error(msg));
                                }
                                let mut w = workers.lock().await;
                                if let Some(info) = w.get_mut(&worker_id) {
                                    info.healthy = false;
                                }
                            }
                            3 => { // ChatAction frame
                                info!(target: "SourceWorkerPool",
                                    "Worker {} chat action: {}", worker_id,
                                    String::from_utf8_lossy(&payload));
                            }
                            _ => {
                                warn!(target: "SourceWorkerPool",
                                    "Unknown source frame type {} from worker {}", frame_type, worker_id);
                            }
                        }
                    }
                    Ok(None) => {
                        info!(target: "SourceWorkerPool", "Source worker {} disconnected", worker_id);
                        break;
                    }
                    Err(e) => {
                        warn!(target: "SourceWorkerPool",
                            "Error reading from source worker {}: {}", worker_id, e);
                        break;
                    }
                }
            }

            envelope_opt = task_rx.recv() => {
                match envelope_opt {
                    Some(envelope) => {
                        let msg_id = format!("task-{}", msg_counter);
                        msg_counter += 1;

                        // Encode the source task as an IPC frame
                        let task_json = serde_json::to_value(&envelope.task).unwrap_or_default();
                        let frame = IpcFrame::encode_json(
                            CommandFrameType::Command as u8, &msg_id, &task_json
                        );
                        if let Err(e) = socket.write_all(&frame).await {
                            warn!(target: "SourceWorkerPool",
                                "Failed to send task to source worker {}: {}", worker_id, e);
                            let _ = envelope.response_tx
                                .send(WorkerResult::Error(format!("Send failed: {}", e)));
                            break;
                        }

                        pending.insert(msg_id, envelope.response_tx);
                    }
                    None => {
                        info!(target: "SourceWorkerPool",
                            "Task channel closed for source worker {}", worker_id);
                        break;
                    }
                }
            }
        }
    }

    // Cancel all pending tasks
    for (id, tx) in pending.drain() {
        let _ = tx.send(WorkerResult::Error(format!(
            "Source worker {} disconnected (pending: {})", worker_id, id
        )));
    }

    let mut w = workers.lock().await;
    if let Some(info) = w.get_mut(&worker_id) {
        info.healthy = false;
    }

    info!(target: "SourceWorkerPool", "Source process worker {} handler exiting", worker_id);
}

/// Read one frame from a source worker's socket.
async fn read_one_source_frame<R: AsyncReadExt + Unpin>(
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
