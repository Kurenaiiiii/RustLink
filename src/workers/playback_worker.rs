use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::info;

use crate::config::FadingConfig;
use crate::managers::player_manager::PlayerManager;
use crate::player::worker::WorkerCommand;
use crate::plugins::PluginManager;
use crate::sources::SourceRegistry;
use crate::workers::types::{
    PlaybackWorkerCommand, WorkerCommandEnvelope, WorkerResult, WorkerTaskStats,
};
use dashmap::DashMap;

pub struct PlaybackWorkerTask {
    worker_id: String,
    rx: mpsc::Receiver<WorkerCommandEnvelope>,
    stats_tx: mpsc::UnboundedSender<WorkerTaskStats>,
    players: HashMap<String, PlayerWorkerHandle>,
    started_at: Instant,
    total_commands: u64,
    frames_sent: u64,
    frames_nulled: u64,
    frames_deficit: u64,
    player_manager: Option<Arc<PlayerManager>>,
    sources: Option<SourceRegistry>,
    plugin_manager: Option<Arc<PluginManager>>,
    player_states: DashMap<String, Arc<tokio::sync::RwLock<crate::state::LivePlayerState>>>,
    sponsorblock: DashMap<String, Vec<String>>,
    fade_config: Option<FadingConfig>,
}

struct PlayerWorkerHandle {
    cmd_tx: mpsc::Sender<WorkerCommand>,
    guild_id: String,
    created_at: Instant,
}

impl PlaybackWorkerTask {
    pub fn new(
        worker_id: String,
        rx: mpsc::Receiver<WorkerCommandEnvelope>,
        stats_tx: mpsc::UnboundedSender<WorkerTaskStats>,
    ) -> Self {
        Self {
            worker_id,
            rx,
            stats_tx,
            players: HashMap::new(),
            started_at: Instant::now(),
            total_commands: 0,
            frames_sent: 0,
            frames_nulled: 0,
            frames_deficit: 0,
            player_manager: None,
            sources: None,
            plugin_manager: None,
            player_states: DashMap::new(),
            sponsorblock: DashMap::new(),
            fade_config: None,
        }
    }

    pub fn set_dependencies(
        &mut self,
        player_manager: Arc<PlayerManager>,
        sources: SourceRegistry,
        plugin_manager: Arc<PluginManager>,
        fade_config: FadingConfig,
    ) {
        self.player_manager = Some(player_manager);
        self.sources = Some(sources);
        self.plugin_manager = Some(plugin_manager);
        self.fade_config = Some(fade_config);
    }

    pub async fn run(&mut self) {
        let mut stats_ticker = interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                Some(envelope) = self.rx.recv() => {
                    self.total_commands += 1;
                    let result = self.handle_command(envelope.command).await;
                    if let Some(tx) = envelope.response_tx {
                        let _ = tx.send(result.unwrap_or_else(|e| WorkerResult::Error(e.to_string())));
                    }
                }
                _ = stats_ticker.tick() => {
                    self.report_stats().await;
                }
            }

            if self.rx.is_closed() {
                info!(target: "Worker", "Worker {} shutting down", self.worker_id);
                break;
            }
        }
    }

    async fn handle_command(
        &mut self,
        command: PlaybackWorkerCommand,
    ) -> Result<WorkerResult, anyhow::Error> {
        // Notify plugins of incoming IPC command
        if let Some(pm) = &self.plugin_manager {
            let msg = serde_json::json!({
                "type": "playback_command",
                "worker_id": self.worker_id,
                "command": format!("{:?}", command),
            });
            pm.on_ipc_message(&msg).await;
        }
        match command {
            PlaybackWorkerCommand::CreatePlayer {
                guild_id,
                session_id,
                user_id,
            } => self.handle_create_player(&guild_id, &session_id, &user_id).await,
            PlaybackWorkerCommand::DestroyPlayer { guild_id } => {
                self.handle_destroy_player(&guild_id).await
            }
            PlaybackWorkerCommand::RestorePlayer { guild_id, state } => {
                self.handle_restore_player(&guild_id, &state).await
            }
            PlaybackWorkerCommand::PlayerCommand { guild_id, command } => {
                self.handle_player_command(&guild_id, &command).await
            }
            PlaybackWorkerCommand::LoadTracks { query, source } => {
                self.handle_load_tracks(&query, source.as_deref()).await
            }
            PlaybackWorkerCommand::LoadLyrics {
                title,
                artist,
                identifier,
            } => self.handle_load_lyrics(&title, &artist, identifier.as_deref()).await,
            PlaybackWorkerCommand::LoadMeaning { title, artist } => {
                self.handle_load_meaning(&title, &artist).await
            }
            PlaybackWorkerCommand::GetSources => self.handle_get_sources().await,
            PlaybackWorkerCommand::Ping { timestamp } => {
                Ok(WorkerResult::Pong { timestamp })
            }
            PlaybackWorkerCommand::Shutdown => {
                self.handle_shutdown().await;
                Ok(WorkerResult::Success(serde_json::json!({"shutdown": true})))
            }
            _ => Ok(WorkerResult::Error("Unimplemented".to_string())),
        }
    }

    async fn handle_create_player(
        &mut self,
        guild_id: &str,
        _session_id: &str,
        _user_id: &str,
    ) -> Result<WorkerResult, anyhow::Error> {
        if self.players.contains_key(guild_id) {
            return Ok(WorkerResult::Success(serde_json::json!({
                "status": "already_exists",
                "guildId": guild_id
            })));
        }

        let (tx, _rx) = mpsc::channel(64);
        let handle = PlayerWorkerHandle {
            cmd_tx: tx,
            guild_id: guild_id.to_string(),
            created_at: Instant::now(),
        };
        self.players.insert(guild_id.to_string(), handle);

        info!(target: "Worker", "Player created for guild {} on worker {}", guild_id, self.worker_id);
        Ok(WorkerResult::Success(serde_json::json!({
            "status": "created",
            "guildId": guild_id,
            "workerId": self.worker_id
        })))
    }

    async fn handle_destroy_player(
        &mut self,
        guild_id: &str,
    ) -> Result<WorkerResult, anyhow::Error> {
        self.players.remove(guild_id);
        info!(target: "Worker", "Player destroyed for guild {} on worker {}", guild_id, self.worker_id);
        Ok(WorkerResult::Success(serde_json::json!({
            "status": "destroyed",
            "guildId": guild_id
        })))
    }

    async fn handle_restore_player(
        &mut self,
        guild_id: &str,
        _state: &serde_json::Value,
    ) -> Result<WorkerResult, anyhow::Error> {
        let (tx, _rx) = mpsc::channel(64);
        self.players.insert(
            guild_id.to_string(),
            PlayerWorkerHandle {
                cmd_tx: tx,
                guild_id: guild_id.to_string(),
                created_at: Instant::now(),
            },
        );
        Ok(WorkerResult::Success(serde_json::json!({
            "status": "restored",
            "guildId": guild_id
        })))
    }

    async fn handle_player_command(
        &mut self,
        guild_id: &str,
        _command: &serde_json::Value,
    ) -> Result<WorkerResult, anyhow::Error> {
        if self.players.contains_key(guild_id) {
            Ok(WorkerResult::Success(serde_json::json!({
                "status": "command_sent",
                "guildId": guild_id,
            })))
        } else {
            Ok(WorkerResult::Error(format!("No player for guild {}", guild_id)))
        }
    }

    async fn handle_load_tracks(
        &self,
        _query: &str,
        _source: Option<&str>,
    ) -> Result<WorkerResult, anyhow::Error> {
        Ok(WorkerResult::Error("loadTracks not implemented at worker level".to_string()))
    }

    async fn handle_load_lyrics(
        &self,
        _title: &str,
        _artist: &str,
        _identifier: Option<&str>,
    ) -> Result<WorkerResult, anyhow::Error> {
        Ok(WorkerResult::Error("loadLyrics not implemented at worker level".to_string()))
    }

    async fn handle_load_meaning(
        &self,
        _title: &str,
        _artist: &str,
    ) -> Result<WorkerResult, anyhow::Error> {
        Ok(WorkerResult::Error("loadMeaning not implemented at worker level".to_string()))
    }

    async fn handle_get_sources(&self) -> Result<WorkerResult, anyhow::Error> {
        Ok(WorkerResult::Success(serde_json::json!({
            "sources": []
        })))
    }

    async fn handle_shutdown(&mut self) {
        self.players.clear();
    }

    async fn report_stats(&self) {
        let uptime = self.started_at.elapsed().as_secs();
        let stats = WorkerTaskStats {
            worker_id: self.worker_id.clone(),
            guild_count: self.players.len() as u32,
            playing_count: 0,
            cpu_load: 0.0,
            command_queue_len: 0,
            frames_sent: self.frames_sent,
            frames_nulled: self.frames_nulled,
            frames_deficit: self.frames_deficit,
            memory_used_bytes: 0,
            uptime_secs: uptime,
        };
        let _ = self.stats_tx.send(stats);
    }
}