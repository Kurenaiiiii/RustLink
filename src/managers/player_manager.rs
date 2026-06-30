use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};

use dashmap::DashMap;

use crate::player::player::{Player, PlayerOptions};
use crate::player::worker::{PlayerWorker, RepeatMode, WorkerCommand};
use crate::sources::SourceRegistry;
use crate::plugins::PluginManager;

pub struct PlayerManager {
    guild_workers: Arc<RwLock<HashMap<String, mpsc::Sender<WorkerCommand>>>>,
    players: Arc<DashMap<String, Arc<Mutex<Player>>>>,
    sources: SourceRegistry,
    plugin_manager: Arc<PluginManager>,
    ws_senders: Arc<RwLock<HashMap<String, mpsc::Sender<serde_json::Value>>>>,
    config: PlayerManagerConfig,
}

#[derive(Clone)]
pub struct PlayerManagerConfig {
    pub fade_config: crate::config::FadingConfig,
    pub crossfade_config: crate::config::CrossfadeConfig,
    pub track_stuck_threshold_ms: u64,
    pub player_update_interval: u64,
    pub resample_quality: String,
    pub loudness_normalizer: bool,
    pub lookahead_ms: u64,
    pub gate_threshold_lufs: f64,
    pub sponsorblock_config: crate::config::SponsorBlockConfig,
    pub event_timeout_ms: u64,
    pub max_stuck_recovery_attempts: u32,
}

impl PlayerManager {
    pub fn new(
        sources: SourceRegistry,
        plugin_manager: Arc<PluginManager>,
        config: PlayerManagerConfig,
    ) -> Self {
        Self {
            guild_workers: Arc::new(RwLock::new(HashMap::new())),
            players: Arc::new(DashMap::new()),
            sources,
            plugin_manager,
            ws_senders: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    pub async fn create_player(
        &self,
        guild_id: &str,
        session_id: &str,
        user_id: &str,
        ws_sender: mpsc::Sender<serde_json::Value>,
    ) -> Result<(), String> {
        let mut workers = self.guild_workers.write().await;
        if workers.contains_key(guild_id) {
            return Err("Player already exists for this guild".into());
        }

        let (tx, rx) = mpsc::channel(32);
        self.ws_senders.write().await.insert(guild_id.to_string(), ws_sender.clone());

        let config = self.config.clone();

        // Create the Player state machine
        let player = Arc::new(Mutex::new(Player::new(PlayerOptions {
            guild_id: guild_id.to_string(),
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            ws_sender: ws_sender.clone(),
            sources: self.sources.clone(),
            plugin_manager: self.plugin_manager.clone(),
            default_volume: 100,
            fading: config.fade_config.clone(),
            crossfade: config.crossfade_config.clone(),
            loudness_normalizer: config.loudness_normalizer,
            track_stuck_threshold_ms: config.track_stuck_threshold_ms,
            player_update_interval: config.player_update_interval,
            sponsorblock_config: config.sponsorblock_config.clone(),
            event_timeout_ms: config.event_timeout_ms,
            max_stuck_recovery_attempts: config.max_stuck_recovery_attempts,
        })));
        self.players.insert(guild_id.to_string(), player.clone());

        let sources = self.sources.clone();
        let plugin_manager = self.plugin_manager.clone();
        let guild = guild_id.to_string();

        tokio::spawn(async move {
            let player_states: dashmap::DashMap<String, Arc<RwLock<crate::state::LivePlayerState>>> = dashmap::DashMap::new();
            let sponsorblock_map: dashmap::DashMap<String, Vec<String>> = dashmap::DashMap::new();
            let mut worker = PlayerWorker::new(
                guild.clone(),
                rx,
                Some(ws_sender),
                sources,
                player_states.clone(),
                sponsorblock_map.clone(),
                config.fade_config,
                config.track_stuck_threshold_ms,
                config.resample_quality,
                config.crossfade_config,
                config.loudness_normalizer,
                config.lookahead_ms,
                config.gate_threshold_lufs,
                plugin_manager,
            );
            worker.run().await;
        });

        workers.insert(guild_id.to_string(), tx);

        // Notify plugins
        self.plugin_manager.on_player_create(
            &guild_id.to_string(),
            session_id,
            &serde_json::json!({"guildId": guild_id, "sessionId": session_id}),
        ).await;

        Ok(())
    }

    pub async fn destroy_player(&self, guild_id: &str) {
        let session_id: String = if let Some(entry) = self.players.remove(guild_id) {
            let mut player = entry.1.lock().await;
            let sid = player.session_id.clone();
            player.destroy(true);
            sid
        } else {
            String::new()
        };

        // Notify plugins before removing worker
        self.plugin_manager.on_player_destroy(guild_id, &session_id).await;

        let mut workers = self.guild_workers.write().await;
        if let Some(tx) = workers.remove(guild_id) {
            let _ = tx.send(WorkerCommand::Destroy).await;
        }
        self.ws_senders.write().await.remove(guild_id);
    }

    pub async fn play(&self, guild_id: &str, encoded_track: &str, no_replace: bool) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            entry.value().lock().await.emit_track_end("replaced");
        }
        self.send_command(guild_id, WorkerCommand::Play {
            encoded_track: encoded_track.to_string(),
            no_replace,
        }).await
    }

    pub async fn stop(&self, guild_id: &str) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            entry.value().lock().await.stop();
        }
        self.send_command(guild_id, WorkerCommand::Stop).await
    }

    pub async fn pause(&self, guild_id: &str, state: bool) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            entry.value().lock().await.pause(state);
        }
        self.send_command(guild_id, WorkerCommand::Pause(state)).await
    }

    pub async fn volume(&self, guild_id: &str, vol: u16) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            entry.value().lock().await.volume(vol as u32);
        }
        self.send_command(guild_id, WorkerCommand::Volume(vol)).await
    }

    pub async fn seek(&self, guild_id: &str, position_ms: u64) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            entry.value().lock().await.seek(position_ms).await;
        }
        self.send_command(guild_id, WorkerCommand::Seek(position_ms)).await
    }

    pub async fn set_filters(&self, guild_id: &str, filters: serde_json::Value) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            entry.value().lock().await.set_filters(filters.clone());
        }
        self.send_command(guild_id, WorkerCommand::Filters(filters)).await
    }

    pub async fn next_track(&self, guild_id: &str) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::NextTrack).await
    }

    pub async fn set_repeat(&self, guild_id: &str, mode: RepeatMode) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::Repeat(mode)).await
    }

    pub async fn shuffle(&self, guild_id: &str) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::Shuffle).await
    }

    pub async fn mixer_add_layer(&self, guild_id: &str, name: String, volume: f32, pan: f32) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::MixerAddLayer { name, volume, pan }).await
    }

    pub async fn mixer_remove_layer(&self, guild_id: &str, layer_id: String) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::MixerRemoveLayer { layer_id }).await
    }

    pub async fn mixer_update_layer(
        &self,
        guild_id: &str,
        layer_id: String,
        name: Option<String>,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
    ) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::MixerUpdateLayer {
            layer_id, name, volume, pan, mute, solo,
        }).await
    }

    pub async fn mixer_set_url(&self, guild_id: &str, layer_id: String, url: Option<String>) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::MixerSetUrl { layer_id, url }).await
    }

    pub async fn mixer_list(&self, guild_id: &str) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::MixerList).await
    }

    pub async fn subscribe(&self, guild_id: &str, topic: String) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::Subscribe { topic }).await
    }

    pub async fn unsubscribe(&self, guild_id: &str, topic: String) -> Result<(), String> {
        self.send_command(guild_id, WorkerCommand::Unsubscribe { topic }).await
    }

    pub async fn update_voice(
        &self,
        guild_id: &str,
        session_id: String,
        user_id: String,
        token: String,
        endpoint: String,
    ) -> Result<(), String> {
        if let Some(entry) = self.players.get(guild_id) {
            let voice_payload = crate::state::PlayerVoiceState {
                token: Some(token.clone()),
                endpoint: Some(endpoint.clone()),
                session_id: Some(session_id.clone()),
                channel_id: None,
            };
            entry.value().lock().await.update_voice(voice_payload, false);
        }
        self.send_command(guild_id, WorkerCommand::VoiceUpdate {
            session_id, user_id, token, endpoint,
        }).await
    }

    pub async fn has_player(&self, guild_id: &str) -> bool {
        self.guild_workers.read().await.contains_key(guild_id)
    }

    pub async fn active_count(&self) -> usize {
        self.guild_workers.read().await.len()
    }

    pub async fn active_guilds(&self) -> Vec<String> {
        self.guild_workers.read().await.keys().cloned().collect()
    }

    async fn send_command(&self, guild_id: &str, command: WorkerCommand) -> Result<(), String> {
        let workers = self.guild_workers.read().await;
        match workers.get(guild_id) {
            Some(tx) => tx.send(command).await.map_err(|_| "Player channel closed".into()),
            None => Err("No player found for this guild".into()),
        }
    }
}
