use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{info, warn};

use crate::player::voice::{VoiceConnection, VoiceSession, VoiceSessionInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Destroyed,
}

#[derive(Clone)]
pub struct ConnectionStats {
    pub ping: Arc<AtomicU64>,
    pub reconnect_count: Arc<AtomicU64>,
    pub started_at: Arc<Instant>,
}

impl ConnectionStats {
    fn new() -> Self {
        Self {
            ping: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            started_at: Arc::new(Instant::now()),
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

pub struct ConnectionManager {
    guild_id: String,
    state: Arc<AtomicU32>,
    stats: ConnectionStats,
    cancelled: Arc<AtomicBool>,
    session: tokio::sync::watch::Sender<Option<VoiceSession>>,
    state_receiver: tokio::sync::watch::Receiver<ConnectionState>,
}

impl ConnectionManager {
    pub fn new(guild_id: String) -> Self {
        let (session_tx, _) = tokio::sync::watch::channel(None);
        let (_state_tx, state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
        Self {
            guild_id,
            state: Arc::new(AtomicU32::new(ConnectionState::Disconnected as u32)),
            stats: ConnectionStats::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            session: session_tx,
            state_receiver: state_rx,
        }
    }

    pub fn stats(&self) -> ConnectionStats {
        self.stats.clone()
    }

    pub fn current_state(&self) -> ConnectionState {
        match self.state.load(Ordering::Acquire) {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Reconnecting,
            _ => ConnectionState::Destroyed,
        }
    }

    pub fn watch_state(&self) -> tokio::sync::watch::Receiver<ConnectionState> {
        self.state_receiver.clone()
    }

    pub fn session(&self) -> tokio::sync::watch::Receiver<Option<VoiceSession>> {
        self.session.subscribe()
    }

    fn set_state(&self, new: ConnectionState) {
        self.state.store(new as u32, Ordering::Release);
    }

    pub fn is_connected(&self) -> bool {
        self.current_state() == ConnectionState::Connected
    }

    pub fn start(self: Arc<Self>, conn: VoiceConnection) {
        let this = self;
        tokio::spawn(async move {
            this.run(conn).await;
        });
    }

    pub fn destroy(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.set_state(ConnectionState::Destroyed);
    }

    async fn run(self: Arc<Self>, conn: VoiceConnection) {
        let server_id = conn.guild_id.clone();
        let mut retry_delay = Duration::from_millis(500);

        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }

            if self.current_state() == ConnectionState::Disconnected {
                self.set_state(ConnectionState::Connecting);
            } else {
                self.set_state(ConnectionState::Reconnecting);
                self.stats.reconnect_count.fetch_add(1, Ordering::Relaxed);
            }

            match self.try_connect(&conn).await {
                Ok(session) => {
                    let _ = self.session.send(Some(session));
                    self.set_state(ConnectionState::Connected);
                    retry_delay = Duration::from_millis(500);

                    self.heartbeat_loop(&conn).await;

                    if self.cancelled.load(Ordering::Acquire) {
                        return;
                    }

                    let _ = self.session.send(None);
                    self.set_state(ConnectionState::Disconnected);
                }
                Err(e) => {
                    warn!(target: "Manager", "Connection attempt failed ({}): {e}", server_id);
                }
            }

            if self.cancelled.load(Ordering::Acquire) {
                return;
            }

            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
        }
    }

    async fn try_connect(&self, conn: &VoiceConnection) -> anyhow::Result<VoiceSession> {
        let base_endpoint = conn
            .endpoint
            .split(':')
            .next()
            .unwrap_or(&conn.endpoint)
            .to_string();
        let ws_url = format!("wss://{}?v=4", base_endpoint);

        let (session_tx, session_rx) = oneshot::channel::<VoiceSessionInfo>();

        let ws_url_clone = ws_url.clone();
        let server_id = conn.guild_id.clone();
        let session_id = conn.session_id.clone();
        let user_id = conn.user_id.clone();
        let token = conn.token.clone();
        let cancelled = self.cancelled.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_handshake(
                ws_url_clone, server_id, session_id, user_id, token, session_tx, cancelled,
            )
            .await
            {
                info!(target: "Manager", "Voice handshake ended: {e}");
            }
        });

        let info = tokio::time::timeout(Duration::from_secs(15), session_rx)
            .await
            .map_err(|_| anyhow::anyhow!("Voice session setup timeout"))?
            .map_err(|_| anyhow::anyhow!("Voice session setup failed"))?;

        let address = info.udp_socket.peer_addr()?;
        Ok(VoiceSession {
            udp_socket: Arc::new(info.udp_socket),
            ssrc: info.ssrc,
            secret_key: info.secret_key,
            sequence: 0,
            timestamp: 0,
            address,
            encryption_mode: info.encryption_mode,
        })
    }

    async fn run_handshake(
        ws_url: String,
        server_id: String,
        session_id: String,
        user_id: String,
        token: String,
        session_tx: oneshot::Sender<VoiceSessionInfo>,
        cancelled: Arc<AtomicBool>,
    ) -> anyhow::Result<()> {
        let ws_stream = connect_async(&ws_url).await?.0;
        let (mut write, mut read) = ws_stream.split();

        let identify = json!({
            "op": 0,
            "d": {
                "server_id": server_id,
                "user_id": user_id,
                "session_id": session_id,
                "token": token,
            }
        });

        write
            .send(Message::Text(identify.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send identify: {e}"))?;

        let mut session_info: Option<VoiceSessionInfo> = None;
        let mut session_tx = Some(session_tx);

        loop {
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }

            let msg = tokio::time::timeout(Duration::from_secs(10), read.next()).await;
            let text = match msg {
                Ok(Some(Ok(Message::Text(t)))) => t,
                Ok(Some(Ok(Message::Close(c)))) => {
                    warn!(target: "Manager", "WS closed during handshake: {c:?} (server: {})", server_id);
                    return Ok(());
                }
                Ok(Some(Ok(Message::Ping(d)))) => {
                    let _ = write.send(Message::Pong(d)).await;
                    continue;
                }
                _ => {
                    warn!(target: "Manager", "WS error during handshake (server: {})", server_id);
                    return Ok(());
                }
            };

            let payload: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            match payload["op"].as_u64().unwrap_or(0) {
                2 => {
                    let ip = payload["d"]["ip"].as_str().unwrap_or("").to_string();
                    let port = payload["d"]["port"].as_u64().unwrap_or(0) as u16;
                    let ssrc = payload["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                    info!(target: "Manager", "Ready: UDP {}:{}, SSRC {} (server: {})", ip, port, ssrc, server_id);

                    let udp = UdpSocket::bind("0.0.0.0:0")
                        .await
                        .map_err(|e| anyhow::anyhow!("UDP bind: {e}"))?;
                    let addr: SocketAddr = format!("{}:{}", ip, port)
                        .parse()
                        .map_err(|e| anyhow::anyhow!("Invalid address: {e}"))?;
                    udp.connect(addr).await?;

                    let mut discovery = vec![0u8; 74];
                    discovery[0..2].copy_from_slice(&1u16.to_be_bytes());
                    discovery[2..4].copy_from_slice(&70u16.to_be_bytes());
                    discovery[4..8].copy_from_slice(&ssrc.to_be_bytes());
                    let _ = udp.send(&discovery).await;

                    let mut recv_buf = vec![0u8; 74];
                    let _ =
                        tokio::time::timeout(Duration::from_secs(5), udp.recv(&mut recv_buf)).await;

                    let ip_end = recv_buf[8..72]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(64);
                    let public_ip =
                        String::from_utf8_lossy(&recv_buf[8..8 + ip_end]).to_string();
                    let public_port = u16::from_be_bytes([recv_buf[72], recv_buf[73]]);

                    let select = json!({
                        "op": 1,
                        "d": {
                            "protocol": "udp",
                            "data": {
                                "address": public_ip,
                                "port": public_port,
                                "mode": "xsalsa20_poly1305",
                            },
                        },
                    });

                    if write.send(Message::Text(select.to_string())).await.is_err() {
                        return Ok(());
                    }
                    session_info = Some(VoiceSessionInfo {
                        udp_socket: udp,
                        ssrc,
                        secret_key: [0u8; 32],
                        encryption_mode: crate::player::voice::EncryptionMode::XSalsa20Poly1305,
                    });
                }
                4 => {
                    let key_hex = payload["d"]["secret_key"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_u64().map(|b| b as u8))
                                .collect::<Vec<u8>>()
                        })
                        .unwrap_or_default();

                    if key_hex.len() != 32 {
                        warn!(target: "Manager", "Invalid secret key length: {} (server: {})", key_hex.len(), server_id);
                        return Ok(());
                    }

                    if let Some(ref mut si) = session_info {
                        si.secret_key.copy_from_slice(&key_hex[..32]);
                    }

                    if let Some(tx) = session_tx.take() {
                        if let Some(si) = session_info.take() {
                            let _ = tx.send(si);
                        }
                    }

                    info!(target: "Manager", "Session description received (server: {})", server_id);
                    break;
                }
                6 => {} // heartbeat ack
                8 => {
                    info!(target: "Manager", "Hello received (server: {})", server_id);
                }
                9 => {
                    info!(target: "Manager", "Resume failed (server: {})", server_id);
                    return Ok(());
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn heartbeat_loop(&self, conn: &VoiceConnection) {
        let heartbeat_interval_ms: u64 = 41250;
        let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_interval_ms));
        interval.tick().await;

        let ws_url = format!(
            "wss://{}?v=4",
            conn.endpoint.split(':').next().unwrap_or(&conn.endpoint)
        );

        let ws_stream = match connect_async(&ws_url).await {
            Ok((s, _)) => s,
            Err(_) => return,
        };

        let resume = json!({
            "op": 7,
            "d": {
                "server_id": conn.guild_id,
                "session_id": conn.session_id,
                "token": conn.token,
            }
        });

        let (mut write, mut read) = ws_stream.split();
        if write
            .send(Message::Text(resume.to_string()))
            .await
            .is_err()
        {
            return;
        }

        let mut nonce: u64 = 0;
        let mut missed_acks: u32 = 0;

        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }

            tokio::select! {
                _ = interval.tick() => {
                    nonce = nonce.wrapping_add(1);
                    let hb = json!({"op": 3, "d": nonce});
                    if write.send(Message::Text(hb.to_string())).await.is_err() {
                        return;
                    }
                    missed_acks += 1;
                    if missed_acks > 3 {
                        warn!(target: "Manager", "Missed 3 heartbeats, reconnecting (server: {})", conn.guild_id);
                        return;
                    }
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            let payload: serde_json::Value =
                                serde_json::from_str(&text).unwrap_or_default();
                            match payload["op"].as_u64().unwrap_or(0) {
                                6 => {
                                    missed_acks = 0;
                                }
                                9 => {
                                    info!(target: "Manager", "Resume failed in heartbeat loop (server: {})", conn.guild_id);
                                    return;
                                }
                                _ => {}
                            }
                        }
                        Some(Ok(Message::Ping(d))) => {
                            let _ = write.send(Message::Pong(d)).await;
                        }
                        _ => {
                            return;
                        }
                    }
                }
            }
        }
    }
}
