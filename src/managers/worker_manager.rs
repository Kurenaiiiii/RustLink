use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex, oneshot};

use crate::workers::ipc_transport;
use tokio::time::interval;
use tracing::{error, info, warn};

use crate::config::{ClusterConfig, WorkerProcessMode};
use crate::workers::ipc::{
    CommandFrameType, IpcFrame, IpcHello, RotateSocketMessage, make_worker_socket_path,
};
use crate::workers::playback_worker::PlaybackWorkerTask;
use crate::workers::types::{
    PlaybackWorkerCommand, WorkerCommandEnvelope, WorkerResult, WorkerTaskStats,
};

#[derive(Debug, Clone)]
pub struct WorkerStats {
    pub players: u32,
    pub playing_players: u32,
    pub cpu_load: f64,
    pub event_loop_lag_ms: f64,
    pub frames_sent: u64,
    pub frames_nulled: u64,
    pub frames_deficit: u64,
    pub command_queue_length: u32,
    pub memory_used_bytes: u64,
    pub uptime_secs: u64,
}

impl Default for WorkerStats {
    fn default() -> Self {
        Self {
            players: 0,
            playing_players: 0,
            cpu_load: 0.0,
            event_loop_lag_ms: 0.0,
            frames_sent: 0,
            frames_nulled: 0,
            frames_deficit: 0,
            command_queue_length: 0,
            memory_used_bytes: 0,
            uptime_secs: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub id: String,
    pub pid: u32,
    pub stats: WorkerStats,
    pub healthy: bool,
    pub last_heartbeat: Instant,
    pub started_at: Instant,
    pub guild_count: u32,
}

struct WorkerHandle {
    cmd_tx: mpsc::Sender<WorkerCommandEnvelope>,
    join: Option<tokio::task::JoinHandle<()>>,
}

pub struct WorkerPool {
    workers: Arc<Mutex<HashMap<String, WorkerInfo>>>,
    handles: Arc<Mutex<HashMap<String, WorkerHandle>>>,
    stats_rx: Arc<Mutex<mpsc::UnboundedReceiver<WorkerTaskStats>>>,
    stats_tx: mpsc::UnboundedSender<WorkerTaskStats>,
    guild_to_worker: Arc<Mutex<HashMap<String, String>>>,
    worker_to_guilds: Arc<Mutex<HashMap<String, Vec<String>>>>,
    config: ClusterConfig,
    stopped: Arc<AtomicBool>,
    next_worker_id: Arc<AtomicU32>,
    scaling_locks: Arc<Mutex<ScalingState>>,
    command_timeout_ms: u64,
}

struct ScalingState {
    last_scale_up: Instant,
    last_scale_down: Instant,
}

impl WorkerPool {
    pub fn new(config: ClusterConfig) -> Self {
        let (stats_tx, stats_rx) = mpsc::unbounded_channel();
        let timeout = config.command_timeout;
        Self {
            workers: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
            stats_rx: Arc::new(Mutex::new(stats_rx)),
            stats_tx,
            guild_to_worker: Arc::new(Mutex::new(HashMap::new())),
            worker_to_guilds: Arc::new(Mutex::new(HashMap::new())),
            config,
            stopped: Arc::new(AtomicBool::new(false)),
            next_worker_id: Arc::new(AtomicU32::new(1)),
            scaling_locks: Arc::new(Mutex::new(ScalingState {
                last_scale_up: Instant::now(),
                last_scale_down: Instant::now(),
            })),
            command_timeout_ms: timeout,
        }
    }

    pub async fn start(&self) {
        self.spawn_min_workers().await;
        self.start_stats_collector();
        self.start_health_check();
        self.start_scaling_check();
        info!(target: "WorkerPool", "Worker pool initialized. Min: {}, Max: {}",
            self.config.min_workers, self.config.workers);
    }

    async fn spawn_min_workers(&self) {
        let current = self.workers.lock().await.len();
        let needed = self.config.min_workers.max(1).saturating_sub(current);
        for _ in 0..needed {
            self.spawn_worker().await;
        }
    }

    async fn spawn_worker(&self) -> String {
        match self.config.process_mode {
            WorkerProcessMode::MultiProcess => self.spawn_worker_process().await,
            WorkerProcessMode::InProcess => self.spawn_worker_task().await,
        }
    }

    async fn spawn_worker_task(&self) -> String {
        let id_num = self.next_worker_id.fetch_add(1, Ordering::SeqCst);
        let id = format!("worker-{}", id_num);

        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        let mut worker_task = PlaybackWorkerTask::new(id.clone(), cmd_rx, self.stats_tx.clone());

        let info = WorkerInfo {
            id: id.clone(),
            pid: std::process::id(),
            stats: WorkerStats::default(),
            healthy: true,
            last_heartbeat: Instant::now(),
            started_at: Instant::now(),
            guild_count: 0,
        };

        let join = tokio::spawn(async move {
            worker_task.run().await;
        });

        self.workers.lock().await.insert(id.clone(), info);
        self.handles.lock().await.insert(id.clone(), WorkerHandle {
            cmd_tx,
            join: Some(join),
        });

        info!(target: "WorkerPool", "Spawned in-process worker {} (pid: {})", id_num, std::process::id());
        id
    }

    async fn spawn_worker_process(&self) -> String {
        let id_num = self.next_worker_id.fetch_add(1, Ordering::SeqCst);
        let id = format!("worker-{}", id_num);

        let event_socket_path = make_worker_socket_path("event");
        let command_socket_path = make_worker_socket_path("command");

        let cmd_sock_path = command_socket_path.clone();
        let evt_sock_path = event_socket_path.clone();

        // Start socket servers in background tasks (waits for client connection on Unix)
        let cmd_server: tokio::task::JoinHandle<Option<ipc_transport::ServerStream>> = tokio::spawn(async move {
            match ipc_transport::create_server(&cmd_sock_path).await {
                Ok(stream) => {
                    info!(target: "WorkerPool", "Command socket connected: {}", cmd_sock_path);
                    Some(stream)
                }
                Err(e) => {
                    warn!(target: "WorkerPool", "Failed to create command socket: {}", e);
                    None
                }
            }
        });

        let evt_server: tokio::task::JoinHandle<Option<ipc_transport::ServerStream>> = tokio::spawn(async move {
            match ipc_transport::create_server(&evt_sock_path).await {
                Ok(stream) => {
                    info!(target: "WorkerPool", "Event socket connected: {}", evt_sock_path);
                    Some(stream)
                }
                Err(e) => {
                    warn!(target: "WorkerPool", "Failed to create event socket: {}", e);
                    None
                }
            }
        });

        // Spawn the playback-worker binary
        let exe_name = format!("playback-worker{}", std::env::consts::EXE_SUFFIX);
        let exe_path = std::env::current_exe()
            .map(|p| p.parent().unwrap().join(&exe_name))
            .unwrap_or_else(|_| std::path::PathBuf::from(&exe_name));

        let mut child = match std::process::Command::new(&exe_path)
            .arg("--event-socket")
            .arg(&event_socket_path)
            .arg("--command-socket")
            .arg(&command_socket_path)
            .arg("--worker-id")
            .arg(&id)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(target: "WorkerPool", "Failed to spawn playback-worker process: {}. Falling back to in-process.", e);
                return self.spawn_worker_task().await;
            }
        };

        let pid = child.id();
        let info = WorkerInfo {
            id: id.clone(),
            pid,
            stats: WorkerStats::default(),
            healthy: true,
            last_heartbeat: Instant::now(),
            started_at: Instant::now(),
            guild_count: 0,
        };

        // Accept connections with timeout
        let cmd_client = tokio::time::timeout(Duration::from_secs(10), async {
            cmd_server.await.ok().and_then(|r| r)
        }).await.unwrap_or(None);

        let _evt_client = tokio::time::timeout(Duration::from_secs(10), async {
            evt_server.await.ok().and_then(|r| r)
        }).await.unwrap_or(None);

        // Register worker
        self.workers.lock().await.insert(id.clone(), info);

        if let Some(cmd_client) = cmd_client {
            // Create a channel for sending commands to this process worker
            let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommandEnvelope>(256);

            let worker_id = id.clone();
            let stats_tx = self.stats_tx.clone();
            let workers_ref = self.workers.clone();
            let join = tokio::spawn(async move {
                handle_process_worker(
                    worker_id.clone(),
                    cmd_client,
                    cmd_rx,
                    stats_tx,
                    workers_ref,
                    Duration::from_secs(30),
                )
                .await;
            });

            self.handles.lock().await.insert(id.clone(), WorkerHandle {
                cmd_tx,
                join: Some(join),
            });
            info!(target: "WorkerPool", "Spawned process worker {} (pid: {})", id_num, pid);
        } else {
            warn!(target: "WorkerPool", "Worker {} failed to connect socket, killing process", id);
            let _ = child.kill();
            self.workers.lock().await.remove(&id);
        }

        id
    }

    pub async fn assign_guild(&self, guild_key: &str) -> Result<String, String> {
        let best = self.find_best_worker().await?;
        let mut guild_map = self.guild_to_worker.lock().await;
        guild_map.insert(guild_key.to_string(), best.clone());

        let mut worker_guilds = self.worker_to_guilds.lock().await;
        worker_guilds.entry(best.clone()).or_default().push(guild_key.to_string());

        if let Some(info) = self.workers.lock().await.get_mut(&best) {
            info.guild_count = worker_guilds.get(&best).map(|v| v.len() as u32).unwrap_or(0);
        }

        Ok(best)
    }

    pub async fn unassign_guild(&self, guild_key: &str) {
        let mut guild_map = self.guild_to_worker.lock().await;
        if let Some(worker_id) = guild_map.remove(guild_key) {
            let mut worker_guilds = self.worker_to_guilds.lock().await;
            if let Some(guilds) = worker_guilds.get_mut(&worker_id) {
                guilds.retain(|g| g != guild_key);
            }
            if let Some(info) = self.workers.lock().await.get_mut(&worker_id) {
                info.guild_count = worker_guilds
                    .get(&worker_id)
                    .map(|v| v.len() as u32)
                    .unwrap_or(0);
            }
        }
    }

    pub async fn get_worker_for_guild(&self, guild_key: &str) -> Option<String> {
        self.guild_to_worker.lock().await.get(guild_key).cloned()
    }

    /// Send a command to a specific worker and wait for the result.
    pub async fn send_command(
        &self,
        worker_id: &str,
        command: PlaybackWorkerCommand,
    ) -> Result<WorkerResult, String> {
        let handles = self.handles.lock().await;
        let handle = handles
            .get(worker_id)
            .ok_or_else(|| format!("Worker {} not found", worker_id))?;

        let (tx, rx) = oneshot::channel();
        let envelope = WorkerCommandEnvelope {
            command,
            response_tx: Some(tx),
        };

        handle
            .cmd_tx
            .send(envelope)
            .await
            .map_err(|_| format!("Failed to send command to worker {}", worker_id))?;

        let timeout = Duration::from_millis(self.command_timeout_ms);
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| format!("Command timed out on worker {}", worker_id))?
            .map_err(|_| format!("Worker {} response channel closed", worker_id))
    }

    /// Send a fire-and-forget command (no response expected).
    pub async fn send_command_ff(
        &self,
        worker_id: &str,
        command: PlaybackWorkerCommand,
    ) -> Result<(), String> {
        let handles = self.handles.lock().await;
        let handle = handles
            .get(worker_id)
            .ok_or_else(|| format!("Worker {} not found", worker_id))?;

        let envelope = WorkerCommandEnvelope {
            command,
            response_tx: None,
        };

        handle
            .cmd_tx
            .send(envelope)
            .await
            .map_err(|_| format!("Failed to send command to worker {}", worker_id))
    }

    pub async fn update_stats(&self, worker_id: &str, stats: WorkerStats) {
        if let Some(info) = self.workers.lock().await.get_mut(worker_id) {
            info.stats = stats;
            info.last_heartbeat = Instant::now();
        }
    }

    pub async fn mark_unhealthy(&self, worker_id: &str) {
        if let Some(info) = self.workers.lock().await.get_mut(worker_id) {
            info.healthy = false;
        }
    }

    pub async fn remove_worker(&self, worker_id: &str) {
        let mut handles = self.handles.lock().await;
        if let Some(mut handle) = handles.remove(worker_id) {
            if let Some(join) = handle.join.take() {
                let _ = handle.cmd_tx.try_send(WorkerCommandEnvelope {
                    command: PlaybackWorkerCommand::Shutdown,
                    response_tx: None,
                });
                join.abort();
            }
        }
        drop(handles);

        let mut workers = self.workers.lock().await;
        workers.remove(worker_id);
        let mut guild_map = self.guild_to_worker.lock().await;
        guild_map.retain(|_, v| v != worker_id);
        let mut worker_guilds = self.worker_to_guilds.lock().await;
        worker_guilds.remove(worker_id);
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

    pub async fn all_workers(&self) -> Vec<WorkerInfo> {
        self.workers.lock().await.values().cloned().collect()
    }

    pub async fn total_guilds(&self) -> usize {
        self.guild_to_worker.lock().await.len()
    }

    pub async fn get_worker_metrics(&self) -> HashMap<String, serde_json::Value> {
        let workers = self.workers.lock().await;
        let mut metrics = HashMap::new();
        for (id, info) in workers.iter() {
            metrics.insert(id.clone(), serde_json::json!({
                "pid": info.pid,
                "healthy": info.healthy,
                "uptime_secs": info.started_at.elapsed().as_secs(),
                "guild_count": info.guild_count,
                "stats": {
                    "players": info.stats.players,
                    "playingPlayers": info.stats.playing_players,
                    "cpuLoad": info.stats.cpu_load,
                    "eventLoopLag": info.stats.event_loop_lag_ms,
                    "framesSent": info.stats.frames_sent,
                    "framesNulled": info.stats.frames_nulled,
                    "commandQueueLength": info.stats.command_queue_length,
                    "memoryUsedBytes": info.stats.memory_used_bytes,
                }
            }));
        }
        metrics
    }

    /// Collect stats from worker tasks and update WorkerInfo.
    fn start_stats_collector(&self) {
        let workers = self.workers.clone();
        let stats_rx = self.stats_rx.clone();
        let stopped = self.stopped.clone();

        tokio::spawn(async move {
            loop {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let mut rx = stats_rx.lock().await;
                let stats = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
                drop(rx);

                match stats {
                    Ok(Some(task_stats)) => {
                        let mut w = workers.lock().await;
                        if let Some(info) = w.get_mut(&task_stats.worker_id) {
                            info.stats.players = task_stats.guild_count;
                            info.stats.playing_players = task_stats.playing_count;
                            info.stats.cpu_load = task_stats.cpu_load;
                            info.stats.command_queue_length = task_stats.command_queue_len;
                            info.stats.frames_sent = task_stats.frames_sent;
                            info.stats.frames_nulled = task_stats.frames_nulled;
                            info.stats.frames_deficit = task_stats.frames_deficit;
                            info.stats.uptime_secs = task_stats.uptime_secs;
                            info.last_heartbeat = Instant::now();
                        }
                    }
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
        });
    }

    async fn find_best_worker(&self) -> Result<String, String> {
        let workers = self.workers.lock().await;
        let mut best: Option<(String, f64)> = None;

        for (id, info) in workers.iter() {
            if !info.healthy {
                continue;
            }
            let cost = self.calculate_cost(info);
            match best {
                Some((_, best_cost)) if cost < best_cost => {
                    best = Some((id.clone(), cost));
                }
                None => {
                    best = Some((id.clone(), cost));
                }
                _ => {}
            }
        }

        if let Some((id, _)) = best {
            return Ok(id);
        }

        drop(workers);
        if self.worker_count().await < self.max_workers().await {
            return Ok(self.spawn_worker().await);
        }

        Err("No healthy workers available".into())
    }

    fn calculate_cost(&self, info: &WorkerInfo) -> f64 {
        let playing_weight = 1.0;
        let paused_weight = 0.01;
        let playing = info.stats.playing_players as f64;
        let paused = info.guild_count.saturating_sub(info.stats.playing_players) as f64;
        let mut cost = playing * playing_weight + paused * paused_weight;

        if info.stats.cpu_load > 0.85 {
            cost += 100.0;
        }
        if info.stats.event_loop_lag_ms > 60.0 {
            cost += 50.0;
        }
        if info.stats.frames_deficit > info.stats.playing_players as u64 * 10 {
            cost += 25.0;
        }

        cost
    }

    async fn max_workers(&self) -> usize {
        if self.config.workers == 0 {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
        } else {
            self.config.workers.max(1)
        }
    }

    fn start_health_check(&self) {
        let workers = self.workers.clone();
        let stopped = self.stopped.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(10));
            loop {
                ticker.tick().await;
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let mut w = workers.lock().await;
                let now = Instant::now();
                w.retain(|id, info| {
                    if !info.healthy && now.duration_since(info.last_heartbeat) > Duration::from_secs(30) {
                        warn!(target: "WorkerPool", "Removing unhealthy worker {}", id);
                        return false;
                    }
                    true
                });
            }
        });
    }

    fn start_scaling_check(&self) {
        let workers = self.workers.clone();
        let handles = self.handles.clone();
        let guild_to_worker = self.guild_to_worker.clone();
        let worker_to_guilds = self.worker_to_guilds.clone();
        let config = self.config.clone();
        let stopped = self.stopped.clone();
        let next_id = self.next_worker_id.clone();
        let scaling_locks = self.scaling_locks.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                if stopped.load(Ordering::Relaxed) {
                    break;
                }

                let w = workers.lock().await;
                let max_workers_val = if config.workers == 0 {
                    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
                } else {
                    config.workers.max(1)
                };

                let active_count = w.values().filter(|wi| wi.healthy).count();
                if active_count == 0 {
                    drop(w);
                    let id_num = next_id.fetch_add(1, Ordering::SeqCst);
                    let worker_id = format!("worker-{}", id_num);
                    let (cmd_tx, cmd_rx) = mpsc::channel(256);
                    let (stats_tx, _stats_rx) = mpsc::unbounded_channel();
                    let mut worker_task = PlaybackWorkerTask::new(worker_id.clone(), cmd_rx, stats_tx);
                    let join = tokio::spawn(async move {
                        worker_task.run().await;
                    });
                    workers.lock().await.insert(worker_id.clone(), WorkerInfo {
                        id: worker_id.clone(),
                        pid: std::process::id(),
                        stats: WorkerStats::default(),
                        healthy: true,
                        last_heartbeat: Instant::now(),
                        started_at: Instant::now(),
                        guild_count: 0,
                    });
                    handles.lock().await.insert(worker_id.clone(), WorkerHandle {
                        cmd_tx,
                        join: Some(join),
                    });
                    continue;
                }

                let total_cost: f64 = w.values().map(|wi| {
                    let playing = wi.stats.playing_players as f64;
                    let paused = wi.guild_count.saturating_sub(wi.stats.playing_players) as f64;
                    playing * 1.0 + paused * 0.01
                }).sum();
                let avg_cost = total_cost / active_count as f64;
                drop(w);

                let max_ppw = max_workers_val.max(20);
                let scale_up_threshold = (max_ppw as f64) * 0.75;
                let scale_down_threshold = 2.0;

                if avg_cost >= scale_up_threshold && active_count < max_workers_val {
                    let mut locks = scaling_locks.lock().await;
                    if locks.last_scale_up.elapsed() > Duration::from_millis(1500) {
                        locks.last_scale_up = Instant::now();
                        drop(locks);
                        let id_num = next_id.fetch_add(1, Ordering::SeqCst);
                        let worker_id = format!("worker-{}", id_num);
                        let (cmd_tx, cmd_rx) = mpsc::channel(256);
                        let (stats_tx, _stats_rx) = mpsc::unbounded_channel();
                        let mut worker_task = PlaybackWorkerTask::new(worker_id.clone(), cmd_rx, stats_tx);
                        let join = tokio::spawn(async move {
                            worker_task.run().await;
                        });
                        workers.lock().await.insert(worker_id.clone(), WorkerInfo {
                            id: worker_id.clone(),
                            pid: std::process::id(),
                            stats: WorkerStats::default(),
                            healthy: true,
                            last_heartbeat: Instant::now(),
                            started_at: Instant::now(),
                            guild_count: 0,
                        });
                        handles.lock().await.insert(worker_id.clone(), WorkerHandle {
                            cmd_tx,
                            join: Some(join),
                        });
                        info!(target: "WorkerPool", "Scaling up: avg_cost={:.2}, active={}, max={}", avg_cost, active_count, max_workers_val);
                    }
                }

                if avg_cost < scale_down_threshold && active_count > config.min_workers.max(1) {
                    let mut locks = scaling_locks.lock().await;
                    if locks.last_scale_down.elapsed() > Duration::from_secs(60) {
                        locks.last_scale_down = Instant::now();
                        drop(locks);
                        let mut h = handles.lock().await;
                        let mut w = workers.lock().await;
                        if let Some(idle) = w.iter()
                            .filter(|(_, wi)| wi.healthy && wi.guild_count == 0)
                            .map(|(id, _)| id.clone())
                            .next()
                        {
                            if let Some(mut handle) = h.remove(&idle) {
                                if let Some(join) = handle.join.take() {
                                    let _ = handle.cmd_tx.try_send(WorkerCommandEnvelope {
                                        command: PlaybackWorkerCommand::Shutdown,
                                        response_tx: None,
                                    });
                                    join.abort();
                                }
                            }
                            w.remove(&idle);
                            drop(w);
                            guild_to_worker.lock().await.retain(|_, v| *v != idle);
                            worker_to_guilds.lock().await.remove(&idle);
                            info!(target: "WorkerPool", "Scaling down: removed idle worker {}", idle);
                        }
                    }
                }
            }
        });
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);

        let mut handles = self.handles.lock().await;
        for (id, mut handle) in handles.drain() {
            let _ = handle.cmd_tx.try_send(WorkerCommandEnvelope {
                command: PlaybackWorkerCommand::Shutdown,
                response_tx: None,
            });
            if let Some(join) = handle.join.take() {
                join.abort();
            }
            info!(target: "WorkerPool", "Shut down worker {}", id);
        }

        self.workers.lock().await.clear();
        self.guild_to_worker.lock().await.clear();
        self.worker_to_guilds.lock().await.clear();
        info!(target: "WorkerPool", "Worker pool shut down");
    }
}

/// Helper: encode a PlaybackWorkerCommand into an IpcFrame BytesMut for sending over the wire.
fn encode_command_frame(msg_id: &str, command: &PlaybackWorkerCommand) -> bytes::BytesMut {
    let payload = serde_json::to_vec(command).unwrap_or_default();
    IpcFrame::encode(CommandFrameType::Command as u8, msg_id, &payload)
}

/// Helper: encode a ping frame.
fn encode_ping_frame(timestamp: u64) -> bytes::BytesMut {
    let payload = serde_json::json!({"type": "ping", "timestamp": timestamp});
    IpcFrame::encode_json(CommandFrameType::Ping as u8, "health", &payload)
}

/// Read one complete frame from an async reader.
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

/// Handle communication with a process-based worker over the command socket.
///
/// This is the core loop that:
/// 1. Reads incoming frames (HELLO, RESULT, ERROR, PONG) from the worker
/// 2. Sends outgoing commands (received via cmd_rx) to the worker
/// 3. Matches RESULT frames to pending command oneshot senders
/// 4. Updates heartbeat on PONG or any activity
        async fn handle_process_worker(
    worker_id: String,
    mut cmd_socket: ipc_transport::ServerStream,
    mut cmd_rx: mpsc::Receiver<WorkerCommandEnvelope>,
    stats_tx: mpsc::UnboundedSender<WorkerTaskStats>,
    workers: Arc<Mutex<HashMap<String, WorkerInfo>>>,
    health_check_timeout: Duration,
) {
    info!(target: "WorkerPool", "Process worker {} connected", worker_id);

    let mut pending: HashMap<String, oneshot::Sender<WorkerResult>> = HashMap::new();
    let mut msg_counter: u64 = 0;
    let mut read_buffer = Vec::with_capacity(65536);
    let mut last_activity = Instant::now();

    loop {
        tokio::select! {
            biased;

            recv_result = read_one_frame(&mut cmd_socket, &mut read_buffer) => {
                match recv_result {
                    Ok(Some((frame_type, id, payload))) => {
                        last_activity = Instant::now();

                        match CommandFrameType::from_u8(frame_type) {
                            Some(CommandFrameType::Hello) => {
                                if let Ok(hello) = serde_json::from_slice::<IpcHello>(&payload) {
                                    info!(target: "WorkerPool", "Worker {} registered: pid={}, type={}",
                                        worker_id, hello.pid, hello.worker_type);
                                    // Update the worker pid in the stats
                                    let mut w = workers.lock().await;
                                    if let Some(info) = w.get_mut(&worker_id) {
                                        info.pid = hello.pid;
                                        info.last_heartbeat = Instant::now();
                                    }
                                }
                            }
                            Some(CommandFrameType::Command) => {
                                // Worker is sending us a command (unexpected for standard flow)
                                warn!(target: "WorkerPool",
                                    "Unexpected command frame from worker {}: id={}", worker_id, id);
                                let error_frame = IpcFrame::encode(
                                    CommandFrameType::Error as u8, &id,
                                    b"{\"error\":\"unexpected command from worker\"}"
                                );
                                let _ = cmd_socket.write_all(&error_frame).await;
                            }
                            Some(CommandFrameType::Result) => {
                                // Forward the result to the pending command
                                if let Some(tx) = pending.remove(&id) {
                                    let result = match serde_json::from_slice::<WorkerResult>(&payload) {
                                        Ok(r) => r,
                                        Err(_) => WorkerResult::Success(
                                            serde_json::from_slice(&payload).unwrap_or_default()
                                        ),
                                    };
                                    let _ = tx.send(result);
                                }
                            }
                            Some(CommandFrameType::Error) => {
                                warn!(target: "WorkerPool", "Worker {} error (id={}): {:?}",
                                    worker_id, id, String::from_utf8_lossy(&payload));
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(WorkerResult::Error(
                                        String::from_utf8_lossy(&payload).to_string()
                                    ));
                                }
                                // Mark worker unhealthy on error frames
                                let mut w = workers.lock().await;
                                if let Some(info) = w.get_mut(&worker_id) {
                                    info.healthy = false;
                                }
                            }
                            Some(CommandFrameType::Pong) => {
                                // Update heartbeat — worker responded to our ping
                                let mut w = workers.lock().await;
                                if let Some(info) = w.get_mut(&worker_id) {
                                    info.last_heartbeat = Instant::now();
                                    info.healthy = true;
                                }
                            }
                            Some(CommandFrameType::Ping) => {
                                // Worker is pinging us — respond with pong
                                let pong_payload = serde_json::json!({
                                    "type": "pong",
                                    "timestamp": serde_json::from_slice::<serde_json::Value>(&payload)
                                        .ok()
                                        .and_then(|v| v["timestamp"].as_u64())
                                        .unwrap_or(0)
                                });
                                let pong_frame = IpcFrame::encode_json(
                                    CommandFrameType::Pong as u8, &id, &pong_payload
                                );
                                let _ = cmd_socket.write_all(&pong_frame).await;
                            }
                            Some(CommandFrameType::RotateSocket) => {
                                if let Ok(msg) = serde_json::from_slice::<RotateSocketMessage>(&payload) {
                                    info!(target: "WorkerPool",
                                        "Worker {} requesting socket rotation: event={}, cmd={}",
                                        worker_id, msg.event_socket_path, msg.command_socket_path);
                                    // The worker will reconnect on new paths — we just acknowledge
                                    let ack = IpcFrame::encode(
                                        CommandFrameType::Result as u8, &id, b"{\"status\":\"ok\"}"
                                    );
                                    let _ = cmd_socket.write_all(&ack).await;
                                }
                            }
                            None => {
                                warn!(target: "WorkerPool",
                                    "Unknown frame type {} from worker {}", frame_type, worker_id);
                            }
                        }
                    }
                    Ok(None) => {
                        info!(target: "WorkerPool", "Process worker {} disconnected", worker_id);
                        break;
                    }
                    Err(e) => {
                        warn!(target: "WorkerPool", "Error reading from worker {}: {}", worker_id, e);
                        break;
                    }
                }
            }

            cmd_opt = cmd_rx.recv() => {
                match cmd_opt {
                    Some(envelope) => {
                        let msg_id = format!("cmd-{}", msg_counter);
                        msg_counter += 1;

                        let frame = encode_command_frame(&msg_id, &envelope.command);
                        if let Err(e) = cmd_socket.write_all(&frame).await {
                            warn!(target: "WorkerPool",
                                "Failed to send command to worker {}: {}", worker_id, e);
                            // If it's a shutdown command with no response, don't propagate error
                            if envelope.response_tx.is_some() {
                                let _ = envelope.response_tx.unwrap()
                                    .send(WorkerResult::Error(format!("Send failed: {}", e)));
                            }
                            break;
                        }

                        if let Some(tx) = envelope.response_tx {
                            pending.insert(msg_id, tx);
                        }
                    }
                    None => {
                        info!(target: "WorkerPool", "Command channel closed for worker {}", worker_id);
                        break;
                    }
                }
            }

            _ = tokio::time::sleep(health_check_timeout) => {
                // Periodic health check: send a ping if no activity recently
                if last_activity.elapsed() > Duration::from_secs(15) {
                    let ping_frame = encode_ping_frame(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64
                    );
                    if let Err(e) = cmd_socket.write_all(&ping_frame).await {
                        warn!(target: "WorkerPool",
                            "Health check ping to worker {} failed: {}", worker_id, e);
                        break;
                    }
                }
            }
        }
    }

    // Mark unhealthy on disconnect
    {
        let mut w = workers.lock().await;
        if let Some(info) = w.get_mut(&worker_id) {
            info.healthy = false;
        }
    }

    // Cancel all pending commands
    for (id, tx) in pending.drain() {
        let _ = tx.send(WorkerResult::Error(format!(
            "Worker {} disconnected (pending: {})", worker_id, id
        )));
    }

    info!(target: "WorkerPool", "Process worker {} handler exiting", worker_id);
}
