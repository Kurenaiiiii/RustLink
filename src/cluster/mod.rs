use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use futures::StreamExt;
use tracing::{error, info, warn};

use crate::config::ClusterConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub node_id: String,
    pub version: String,
    pub address: String,
    pub players: usize,
    pub playing_players: usize,
    pub uptime_secs: u64,
    pub last_heartbeat: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClusterMessage {
    PlayerMove {
        guild_id: String,
        from_node: String,
        to_node: String,
    },
    StatsUpdate {
        node_id: String,
        players: usize,
        playing_players: usize,
        uptime_secs: u64,
    },
    SessionResume {
        session_id: String,
        guild_id: String,
        from_node: String,
    },
    NodeShutdown {
        node_id: String,
    },
}

pub struct ClusterManager {
    node_id: String,
    redis_url: String,
    config: ClusterConfig,
    conn: Arc<Mutex<Option<ConnectionManager>>>,
    _start_time: tokio::time::Instant,
    player_count: Arc<Mutex<(usize, usize)>>,
    nodes: Arc<Mutex<Vec<ClusterNode>>>,
}

impl ClusterManager {
    pub fn new(config: &ClusterConfig) -> Self {
        let node_id = config
            .node_id
            .clone()
            .unwrap_or_else(|| {
                let short = uuid::Uuid::new_v4().to_string();
                format!("node-{}", &short[..8])
            });

        let redis_url = config
            .redis_url
            .clone()
            .unwrap_or_else(|| "redis://127.0.0.1:6379".to_string());

        Self {
            node_id,
            redis_url,
            config: config.clone(),
            conn: Arc::new(Mutex::new(None)),
            _start_time: tokio::time::Instant::now(),
            player_count: Arc::new(Mutex::new((0, 0))),
            nodes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[allow(dead_code)]
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub async fn start(&self) {
        let redis_url = self.redis_url.clone();
        let client = match redis::Client::open(redis_url.as_str()) {
            Ok(c) => c,
            Err(e) => {
                error!(target: "Cluster", "Invalid Redis URL: {}", e);
                return;
            }
        };

        let conn = match ConnectionManager::new(client.clone()).await {
            Ok(c) => c,
            Err(e) => {
                error!(target: "Cluster", "Failed to connect to Redis: {}", e);
                return;
            }
        };

        {
            let mut c = self.conn.lock().await;
            *c = Some(conn);
        }

        self.register_node().await;
        self.start_heartbeat();
        self.start_ipc_listener(client);
        self.start_node_watcher();
    }

    async fn register_node(&self) {
        let conn_locked = self.conn.lock().await;
        let conn = match conn_locked.as_ref() {
            Some(c) => c,
            None => return,
        };
        let mut conn = conn.clone();

        let node_key = format!("rustlink:cluster:node:{}", self.node_id);
        let node_info = ClusterNode {
            node_id: self.node_id.clone(),
            version: "3.8.0".to_string(),
            address: String::new(),
            players: 0,
            playing_players: 0,
            uptime_secs: 0,
            last_heartbeat: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        let json = serde_json::to_string(&node_info).unwrap_or_default();
        let ttl = (self.config.node_timeout_secs + 10) as u64;

        if let Err(e) = conn
            .set_ex::<_, _, ()>(node_key.clone(), json.as_str(), ttl)
            .await
        {
            error!(target: "Cluster", "Failed to register node in Redis: {}", e);
            return;
        }

        if let Err(e) = conn
            .sadd::<_, _, ()>("rustlink:cluster:nodes", self.node_id.as_str())
            .await
        {
            error!(target: "Cluster", "Failed to add node to set: {}", e);
        }

        info!(target: "Cluster", "Registered node '{}' in cluster (ttl: {}s)", self.node_id, ttl);
    }

    fn start_heartbeat(&self) {
        let node_id = self.node_id.clone();
        let interval_secs = self.config.heartbeat_interval_secs;
        let node_timeout = self.config.node_timeout_secs;
        let conn_locked = self.conn.clone();
        let player_count = self.player_count.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            loop {
                ticker.tick().await;
                let conn_lk = conn_locked.lock().await;
                let conn = match conn_lk.as_ref() {
                    Some(c) => c,
                    None => continue,
                };
                let mut conn = conn.clone();
                drop(conn_lk);

                let node_key = format!("rustlink:cluster:node:{}", node_id);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let ttl = (node_timeout + 10) as u64;

                let (players, playing) = *player_count.lock().await;

                let node_info = ClusterNode {
                    node_id: node_id.clone(),
                    version: "3.8.0".to_string(),
                    address: String::new(),
                    players,
                    playing_players: playing,
                    uptime_secs: 0,
                    last_heartbeat: now,
                };

                let json = serde_json::to_string(&node_info).unwrap_or_default();

                if let Err(e) = conn
                    .set_ex::<_, _, ()>(node_key, json.as_str(), ttl)
                    .await
                {
                    warn!(target: "Cluster", "Heartbeat update failed: {}", e);
                }
            }
        });
    }

    fn start_ipc_listener(&self, client: redis::Client) {
        let nodes = self.nodes.clone();

        tokio::spawn(async move {
            loop {
                #[allow(deprecated)]
                let conn = match client.get_async_connection().await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(target: "Cluster", "IPC connection failed: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };
                let mut ps = conn.into_pubsub();

                if let Err(e) = ps.subscribe("rustlink:cluster:ipc").await {
                    warn!(target: "Cluster", "IPC subscribe failed: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }

                info!(target: "Cluster", "IPC listener started on rustlink:cluster:ipc");

                let mut stream = ps.on_message();
                while let Some(msg) = stream.next().await {
                    let payload: String = match msg.get_payload() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if let Ok(cmd) = serde_json::from_str::<ClusterMessage>(&payload) {
                        let mut n = nodes.lock().await;
                        match cmd {
                            ClusterMessage::StatsUpdate {
                                node_id,
                                players,
                                playing_players,
                                ..
                            } => {
                                if let Some(existing) = n.iter_mut().find(|n: &&mut ClusterNode| n.node_id == node_id) {
                                    existing.players = players;
                                    existing.playing_players = playing_players;
                                } else {
                                    n.push(ClusterNode {
                                        node_id,
                                        version: "3.8.0".to_string(),
                                        address: String::new(),
                                        players,
                                        playing_players,
                                        uptime_secs: 0,
                                        last_heartbeat: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs(),
                                    });
                                }
                            }
                            ClusterMessage::NodeShutdown { node_id } => {
                                n.retain(|node| node.node_id != node_id);
                                info!(target: "Cluster", "Node '{}' left the cluster", node_id);
                            }
                            _ => {}
                        }
                    }
                }
            }
        });
    }

    fn start_node_watcher(&self) {
        let conn_locked = self.conn.clone();
        let nodes = self.nodes.clone();
        let interval_secs = self.config.heartbeat_interval_secs.max(5);

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            loop {
                ticker.tick().await;
                let conn_lk = conn_locked.lock().await;
                let conn = match conn_lk.as_ref() {
                    Some(c) => c,
                    None => continue,
                };
                let mut conn = conn.clone();
                drop(conn_lk);

                let member_ids: Vec<String> = match conn.smembers("rustlink:cluster:nodes").await {
                    Ok(ids) => ids,
                    Err(_) => continue,
                };

                let mut current_nodes = Vec::new();
                for member_id in &member_ids {
                    let node_key = format!("rustlink:cluster:node:{}", member_id);
                    let data: Option<String> = conn.get(&node_key).await.unwrap_or(None);
                    if let Some(json) = data {
                        if let Ok(node) = serde_json::from_str::<ClusterNode>(&json) {
                            current_nodes.push(node);
                        }
                    }
                }

                let mut n = nodes.lock().await;
                *n = current_nodes;
            }
        });
    }

    #[allow(dead_code)]
    pub async fn publish_message(&self, msg: &ClusterMessage) {
        let conn_lk = self.conn.lock().await;
        let conn = match conn_lk.as_ref() {
            Some(c) => c,
            None => return,
        };
        let mut conn = conn.clone();
        drop(conn_lk);

        let payload = serde_json::to_string(msg).unwrap_or_default();
        if let Err(e) = conn
            .publish::<_, _, ()>("rustlink:cluster:ipc", payload.as_str())
            .await
        {
            warn!(target: "Cluster", "Failed to publish IPC message: {}", e);
        }
    }

    pub async fn update_player_count(&self, players: usize, playing: usize) {
        let mut pc = self.player_count.lock().await;
        *pc = (players, playing);
    }

    #[allow(dead_code)]
    pub async fn get_nodes(&self) -> Vec<ClusterNode> {
        self.nodes.lock().await.clone()
    }
}
