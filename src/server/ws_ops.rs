use crate::player::worker::{PlayerWorker, WorkerCommand};
use crate::state::SharedState;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info};

#[derive(Deserialize, Debug)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum LavalinkOp {
    VoiceUpdate {
        guild_id: String,
        session_id: String,
        event: VoiceEvent,
    },
    Play {
        guild_id: String,
        track: TrackData,
        #[serde(default)]
        _start_time: Option<i64>,
        #[serde(default)]
        _end_time: Option<i64>,
        #[serde(default)]
        no_replace: Option<bool>,
        #[serde(default)]
        _pause: Option<bool>,
    },
    Stop {
        guild_id: String,
    },
    Pause {
        guild_id: String,
        pause: bool,
    },
    Seek {
        guild_id: String,
        position: u64,
    },
    Volume {
        guild_id: String,
        volume: u16,
    },
    Filters {
        guild_id: String,
        filters: serde_json::Value,
    },
    ConfigureResuming {
        #[serde(default)]
        _key: Option<String>,
        timeout: u64,
    },
    Destroy {
        guild_id: String,
    },
    Mixer {
        guild_id: String,
        action: String,
        #[serde(default)]
        layer_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        volume: Option<f32>,
        #[serde(default)]
        pan: Option<f32>,
        #[serde(default)]
        mute: Option<bool>,
        #[serde(default)]
        solo: Option<bool>,
        #[serde(default)]
        url: Option<String>,
    },
    Subscribe {
        guild_id: String,
        topic: String,
    },
    Unsubscribe {
        guild_id: String,
        topic: String,
    },
}

#[derive(Deserialize, Debug)]
pub struct VoiceEvent {
    pub token: String,
    pub endpoint: String,
}

#[derive(Deserialize, Debug)]
pub struct TrackData {
    pub encoded: String,
}

pub async fn handle_op(state: &SharedState, current_session_id: &str, op: LavalinkOp) {
    match op {
        LavalinkOp::ConfigureResuming { _key: _, timeout } => {
            let mut session = state.sessions.entry(current_session_id.to_string()).or_insert(
                crate::state::Session {
                    id: current_session_id.to_string(),
                    user_id: "0".into(),
                    resuming: true,
                    timeout,
                    players: Vec::new(),
                },
            );
            session.resuming = true;
            session.timeout = timeout;
            info!(
                target: "NodeLink",
                "Session {} configured resuming with timeout {}",
                current_session_id, timeout
            );
            return;
        }
        _ => {}
    }

    let (guild_id, cmd) = match op {
        LavalinkOp::VoiceUpdate {
            guild_id,
            session_id,
            event,
        } => {
            let user_id = state
                .sessions
                .get(current_session_id)
                .map(|s| s.user_id.clone())
                .unwrap_or_else(|| "0".into());
            state.plugin_manager.on_voice_server_update(&guild_id, &event.endpoint, &event.token).await;
            (
                guild_id,
                WorkerCommand::VoiceUpdate {
                    session_id,
                    user_id,
                    token: event.token,
                    endpoint: event.endpoint,
                },
            )
        }
        LavalinkOp::Play {
            guild_id,
            track,
            _start_time: _,
            _end_time: _,
            no_replace,
            _pause: _,
        } => (
            guild_id,
            WorkerCommand::Play {
                encoded_track: track.encoded,
                no_replace: no_replace.unwrap_or(false),
            },
        ),
        LavalinkOp::Stop { guild_id } => (guild_id, WorkerCommand::Stop),
        LavalinkOp::Pause { guild_id, pause } => (guild_id, WorkerCommand::Pause(pause)),
        LavalinkOp::Seek { guild_id, position } => (guild_id, WorkerCommand::Seek(position)),
        LavalinkOp::Volume { guild_id, volume } => (guild_id, WorkerCommand::Volume(volume)),
        LavalinkOp::Filters {
            guild_id,
            filters,
        } => (guild_id, WorkerCommand::Filters(filters)),
        LavalinkOp::Mixer { guild_id, action, layer_id, name, volume, pan, mute, solo, url } => {
            let cmd = match action.as_str() {
                "addLayer" => WorkerCommand::MixerAddLayer {
                    name: name.unwrap_or_default(),
                    volume: volume.unwrap_or(1.0),
                    pan: pan.unwrap_or(0.0),
                },
                "removeLayer" => WorkerCommand::MixerRemoveLayer {
                    layer_id: layer_id.unwrap_or_default(),
                },
                "updateLayer" => WorkerCommand::MixerUpdateLayer {
                    layer_id: layer_id.unwrap_or_default(),
                    name, volume, pan, mute, solo,
                },
                "setUrl" => WorkerCommand::MixerSetUrl {
                    layer_id: layer_id.unwrap_or_default(),
                    url,
                },
                _ => WorkerCommand::MixerList,
            };
            (guild_id, cmd)
        }
        LavalinkOp::Subscribe { guild_id, topic } => (guild_id, WorkerCommand::Subscribe { topic }),
        LavalinkOp::Unsubscribe { guild_id, topic } => (guild_id, WorkerCommand::Unsubscribe { topic }),
        LavalinkOp::Destroy { guild_id } => (guild_id, WorkerCommand::Destroy),
        LavalinkOp::ConfigureResuming { .. } => unreachable!(),
    };

    // If worker doesn't exist for this guild, spawn it!
    let mut workers = state.workers.write().await;
    if !workers.contains_key(&guild_id) {
        info!(
            target: "NodeLink",
            "Spawning new isolated worker for Guild {}",
            guild_id
        );

        let (tx, rx) = mpsc::channel(100);

        let ws_senders = state.ws_senders.read().await;
        let ws_sender = ws_senders.get(current_session_id).cloned();

        // Clone ws_sender before moving into worker
        let ws_sender_for_event = ws_sender.clone();

        let worker = PlayerWorker::new(
            guild_id.clone(),
            rx,
            ws_sender,
            state.sources.clone(),
            state.player_states.clone(),
            state.sponsorblock.clone(),
            state.config.audio.fading.clone(),
            state.config.track_stuck_threshold_ms,
            state.config.audio.resampling_quality.clone(),
            state.config.audio.crossfade.clone(),
            state.config.audio.loudness_normalizer,
            state.config.audio.lookahead_ms,
            state.config.audio.gate_threshold_lufs,
            state.plugin_manager.clone(),
        );

        // Emit PlayerCreated event
        if let Some(ref ws_sender) = ws_sender_for_event {
            let _ = ws_sender.try_send(serde_json::json!({
                "op": "event",
                "type": crate::constants::gateway_events::PLAYER_CREATED,
                "guildId": guild_id
            }));
        }

        tokio::spawn(async move {
            worker.run().await;
        });

        workers.insert(guild_id.clone(), tx);
    }

    if let Some(tx) = workers.get(&guild_id) {
        if let Err(e) = tx.send(cmd).await {
            error!(
                target: "NodeLink",
                "Failed to send command to worker {}: {}",
                guild_id, e
            );
        }
    }
}
