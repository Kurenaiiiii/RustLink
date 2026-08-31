use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use davey::{CommitWelcome, DaveSession, ProposalsOperationType};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, MaybeTlsStream, tungstenite::protocol::Message};
use tokio_tungstenite::WebSocketStream;
use tracing::{error, info, warn};

use crate::voice::relay::{InterceptedPacket, RelayConfig, VoiceRelay};

// ---------------------------------------------------------------------------
// Encryption mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionMode {
    XSalsa20Poly1305,
    XSalsa20Poly1305Suffix,
    XSalsa20Poly1305Lite,
    AeadAes256Gcm,
    AeadAes256GcmRtpsize,
}

impl EncryptionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::XSalsa20Poly1305 => "xsalsa20_poly1305",
            Self::XSalsa20Poly1305Suffix => "xsalsa20_poly1305_suffix",
            Self::XSalsa20Poly1305Lite => "xsalsa20_poly1305_lite",
            Self::AeadAes256Gcm => "aead_aes256_gcm",
            Self::AeadAes256GcmRtpsize => "aead_aes256_gcm_rtpsize",
        }
    }

    pub fn nonce_size(&self) -> usize {
        match self {
            Self::XSalsa20Poly1305 => 24,
            Self::XSalsa20Poly1305Suffix => 24,
            Self::XSalsa20Poly1305Lite => 4,
            Self::AeadAes256Gcm => 12,
            Self::AeadAes256GcmRtpsize => 4,
        }
    }

    pub fn suffix_nonce(&self) -> bool {
        matches!(self, Self::XSalsa20Poly1305Suffix | Self::AeadAes256Gcm)
    }

    pub fn all_supported() -> Vec<EncryptionMode> {
        vec![
            EncryptionMode::XSalsa20Poly1305,
            EncryptionMode::XSalsa20Poly1305Lite,
            EncryptionMode::XSalsa20Poly1305Suffix,
            EncryptionMode::AeadAes256Gcm,
            EncryptionMode::AeadAes256GcmRtpsize,
        ]
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "xsalsa20_poly1305" => Some(Self::XSalsa20Poly1305),
            "xsalsa20_poly1305_suffix" => Some(Self::XSalsa20Poly1305Suffix),
            "xsalsa20_poly1305_lite" => Some(Self::XSalsa20Poly1305Lite),
            "aead_aes256_gcm" => Some(Self::AeadAes256Gcm),
            "aead_aes256_gcm_rtpsize" => Some(Self::AeadAes256GcmRtpsize),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DAVE protocol constants
// ---------------------------------------------------------------------------

pub const DAVE_PROTOCOL_MAX: u16 = 1;
const MLS_OPCODE_EXTERNAL_SENDER: u8 = 25;
const MLS_OPCODE_KEY_PACKAGE: u8 = 26;
const MLS_OPCODE_PROPOSALS: u8 = 27;
const MLS_OPCODE_COMMIT_WELCOME: u8 = 28;
const MLS_OPCODE_ANNOUNCE_COMMIT: u8 = 29;
const MLS_OPCODE_WELCOME: u8 = 30;
const MLS_OPCODE_INVALID_COMMIT: u8 = 31;

pub type DaveHandle = Arc<Mutex<Option<DaveSession>>>;

pub fn new_dave_handle() -> DaveHandle {
    Arc::new(Mutex::new(None))
}

#[allow(dead_code)]
fn encode_mls_varint(value: u32) -> Vec<u8> {
    if value < (1 << 6) {
        vec![value as u8]
    } else if value < (1 << 14) {
        vec![0x40 | (value >> 8) as u8, value as u8]
    } else {
        vec![
            0x80 | (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]
    }
}

#[allow(dead_code)]
fn decode_mls_varint(data: &[u8]) -> Option<(u32, usize)> {
    if data.is_empty() {
        return None;
    }
    let first = data[0] as u32;
    if first < 0x40 {
        Some((first, 1))
    } else if first < 0x80 {
        if data.len() < 2 {
            return None;
        }
        let value = ((first & 0x3f) << 8) | data[1] as u32;
        Some((value, 2))
    } else {
        if data.len() < 4 {
            return None;
        }
        let value = ((first & 0x3f) << 24)
            | (data[1] as u32) << 16
            | (data[2] as u32) << 8
            | data[3] as u32;
        Some((value, 4))
    }
}

pub type WsWrite =
    futures::stream::SplitSink<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Message>;

// ---------------------------------------------------------------------------
// State enums (matching NodeLink's VoiceConnectionState / VoicePlayerState)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Destroyed,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoicePlayerStatus {
    Idle,
    Playing,
    Paused,
}

#[derive(Debug, Clone)]
pub struct VoiceConnectionState {
    pub status: VoiceConnectionStatus,
    pub code: Option<u32>,
    pub reason: Option<String>,
}

impl VoiceConnectionState {
    pub fn new(status: VoiceConnectionStatus) -> Self {
        Self { status, code: None, reason: None }
    }
}

#[derive(Debug, Clone)]
pub struct VoicePlayerState {
    pub status: VoicePlayerStatus,
    pub reason: Option<String>,
}

impl VoicePlayerState {
    pub fn new(status: VoicePlayerStatus) -> Self {
        Self { status, reason: None }
    }
}

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum VoiceConnectionEvent {
    StateChange {
        old: VoiceConnectionState,
        new: VoiceConnectionState,
    },
    PlayerStateChange {
        old: VoicePlayerState,
        new: VoicePlayerState,
    },
    Error(String),
    SpeakStart {
        user_id: String,
        ssrc: u32,
    },
    SpeakEnd {
        user_id: String,
        ssrc: u32,
    },
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct VoiceStatistics {
    pub packets_sent: u64,
    pub packets_expected: u64,
    pub packets_lost: u64,
    pub bytes_sent: u64,
}

// ---------------------------------------------------------------------------
// VoiceAudioResource trait (matching NodeLink's AudioResource)
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait VoiceAudioResource: Send + Sync {
    async fn next_opus_frame(&mut self) -> Option<Vec<u8>>;
    fn position_ms(&self) -> u64;
    fn set_volume(&mut self, _volume: f32) {}
    fn destroy(&mut self) {}
}

// ---------------------------------------------------------------------------
// VoiceSession (low-level UDP send)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct VoiceSession {
    pub udp_socket: Arc<UdpSocket>,
    pub user_id: u64,
    pub ssrc: u32,
    pub secret_key: [u8; 32],
    pub sequence: u16,
    pub timestamp: u32,
    pub address: SocketAddr,
    pub encryption_mode: EncryptionMode,
    pub dave_session: DaveHandle,
}

fn encrypt_xsalsa20(
    opus_data: &[u8],
    secret_key: &[u8; 32],
    sequence: u16,
    _timestamp: u32,
    _ssrc: u32,
    mode: EncryptionMode,
    packet: &mut Vec<u8>,
) -> anyhow::Result<()> {
    let mut nonce = [0u8; 24];
    match mode {
        EncryptionMode::XSalsa20Poly1305 => {
            nonce[..4].copy_from_slice(&sequence.to_le_bytes());
        }
        EncryptionMode::XSalsa20Poly1305Suffix => {
            getrandom::getrandom(&mut nonce)?;
        }
        EncryptionMode::XSalsa20Poly1305Lite => {
            nonce[..4].copy_from_slice(&sequence.to_le_bytes());
        }
        _ => unreachable!(),
    }

    let cipher = xsalsa20poly1305::XSalsa20Poly1305::new_from_slice(secret_key)
        .map_err(|e| anyhow::anyhow!("Invalid key: {e:?}"))?;

    let encrypted = cipher
        .encrypt(
            &nonce.into(),
            Payload {
                msg: opus_data,
                aad: &[],
            },
        )
        .map_err(|e| anyhow::anyhow!("Encrypt failed: {e:?}"))?;

    packet.extend_from_slice(&encrypted);
    if mode == EncryptionMode::XSalsa20Poly1305Suffix {
        packet.extend_from_slice(&nonce);
    } else if mode == EncryptionMode::XSalsa20Poly1305Lite {
        packet.extend_from_slice(&nonce[..4]);
    }

    Ok(())
}

fn encrypt_aes256_gcm(
    opus_data: &[u8],
    secret_key: &[u8; 32],
    sequence: u16,
    _timestamp: u32,
    _ssrc: u32,
    mode: EncryptionMode,
    rtp_header: &[u8],
    packet: &mut Vec<u8>,
) -> anyhow::Result<()> {
    let (nonce_bytes, include_suffix) = match mode {
        EncryptionMode::AeadAes256Gcm => {
            let mut n = [0u8; 12];
            getrandom::getrandom(&mut n)?;
            (n.to_vec(), true)
        }
        EncryptionMode::AeadAes256GcmRtpsize => {
            let mut n = [0u8; 4];
            n.copy_from_slice(&sequence.to_le_bytes());
            (n.to_vec(), false)
        }
        _ => unreachable!(),
    };

    let cipher = Aes256Gcm::new_from_slice(secret_key)
        .map_err(|e| anyhow::anyhow!("Invalid AES key: {e:?}"))?;

    let aad = if mode == EncryptionMode::AeadAes256GcmRtpsize {
        rtp_header
    } else {
        &[]
    };

    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: opus_data,
                aad,
            },
        )
        .map_err(|e| anyhow::anyhow!("AES encrypt failed: {e:?}"))?;

    packet.extend_from_slice(&encrypted);
    if include_suffix {
        packet.extend_from_slice(&nonce_bytes);
    }

    Ok(())
}

impl VoiceSession {
    pub async fn send_opus_frame(&mut self, opus_data: &[u8]) -> anyhow::Result<()> {
        // Step A: DAVE E2EE encrypt (if session is ready)
        let payload = {
            let mut ds = self.dave_session.lock().await;
            match *ds {
                Some(ref mut session) if session.is_ready() => session
                    .encrypt_opus(opus_data)
                    .map(|cow| cow.into_owned())
                    .unwrap_or_else(|e| {
                        warn!(target: "Voice", "DAVE encrypt failed: {e}");
                        opus_data.to_vec()
                    }),
                _ => opus_data.to_vec(),
            }
        };

        let sequence = self.sequence;
        let timestamp = self.timestamp;
        let ssrc = self.ssrc;

        let mut rtp_header = [0u8; 12];
        rtp_header[0] = 0x80;
        rtp_header[1] = 0x78;
        rtp_header[2..4].copy_from_slice(&sequence.to_be_bytes());
        rtp_header[4..8].copy_from_slice(&timestamp.to_be_bytes());
        rtp_header[8..12].copy_from_slice(&ssrc.to_be_bytes());

        let mut packet = Vec::with_capacity(rtp_header.len() + payload.len() + 24);
        packet.extend_from_slice(&rtp_header);

        match self.encryption_mode {
            EncryptionMode::XSalsa20Poly1305
            | EncryptionMode::XSalsa20Poly1305Suffix
            | EncryptionMode::XSalsa20Poly1305Lite => {
                encrypt_xsalsa20(
                    &payload,
                    &self.secret_key,
                    sequence,
                    timestamp,
                    ssrc,
                    self.encryption_mode,
                    &mut packet,
                )?;
            }
            EncryptionMode::AeadAes256Gcm | EncryptionMode::AeadAes256GcmRtpsize => {
                encrypt_aes256_gcm(
                    &payload,
                    &self.secret_key,
                    sequence,
                    timestamp,
                    ssrc,
                    self.encryption_mode,
                    &rtp_header,
                    &mut packet,
                )?;
            }
        }

        self.udp_socket.send_to(&packet, self.address).await?;
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(960);

        Ok(())
    }

    pub fn start_relay(&self, config: RelayConfig) -> Option<mpsc::Receiver<InterceptedPacket>> {
        if !config.enabled {
            return None;
        }
        let (tx, rx) = mpsc::channel(config.buffer_size);
        VoiceRelay::start_listener(
            self.udp_socket.clone(),
            self.ssrc,
            self.user_id,
            self.secret_key,
            self.encryption_mode,
            tx,
            Some(self.dave_session.clone()),
        );
        Some(rx)
    }
}

// ---------------------------------------------------------------------------
// VoiceSessionInfo (internal handshake result)
// ---------------------------------------------------------------------------

pub struct VoiceSessionInfo {
    pub udp_socket: Arc<UdpSocket>,
    pub ssrc: u32,
    pub secret_key: [u8; 32],
    pub encryption_mode: EncryptionMode,
}

// ---------------------------------------------------------------------------
// VoiceConnection (high-level, event-emitting, matching @performanc/voice)
// ---------------------------------------------------------------------------

pub struct VoiceConnection {
    pub guild_id: String,
    pub user_id: String,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
    pub channel_id: String,
    preferred_mode: Option<EncryptionMode>,

    session: Arc<Mutex<Option<VoiceSession>>>,
    ws_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    ws_shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,

    event_tx: broadcast::Sender<VoiceConnectionEvent>,

    conn_state: Arc<Mutex<VoiceConnectionState>>,
    player_state: Arc<Mutex<VoicePlayerState>>,

    pub statistics: Arc<Mutex<VoiceStatistics>>,
    pub ping: Arc<AtomicI64>,

    ssrc_map: Arc<Mutex<HashMap<u32, String>>>,

    play_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    play_shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,

    dave_session: DaveHandle,
}

impl VoiceConnection {
    pub fn new(
        guild_id: String,
        user_id: String,
        session_id: String,
        token: String,
        endpoint: String,
        channel_id: String,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(128);
        let conn_state = VoiceConnectionState::new(VoiceConnectionStatus::Disconnected);
        let player_state = VoicePlayerState::new(VoicePlayerStatus::Idle);
        Self {
            guild_id,
            user_id,
            session_id,
            token,
            endpoint,
            channel_id,
            preferred_mode: None,
            session: Arc::new(Mutex::new(None)),
            ws_handle: Arc::new(Mutex::new(None)),
            ws_shutdown: Arc::new(Mutex::new(None)),
            event_tx,
            conn_state: Arc::new(Mutex::new(conn_state)),
            player_state: Arc::new(Mutex::new(player_state)),
            statistics: Arc::new(Mutex::new(VoiceStatistics::default())),
            ping: Arc::new(AtomicI64::new(-1)),
            ssrc_map: Arc::new(Mutex::new(HashMap::new())),
            play_handle: Arc::new(Mutex::new(None)),
            play_shutdown: Arc::new(Mutex::new(None)),
            dave_session: new_dave_handle(),
        }
    }

    pub fn with_mode(mut self, mode: EncryptionMode) -> Self {
        self.preferred_mode = Some(mode);
        self
    }

    pub fn subscribe(&self) -> broadcast::Receiver<VoiceConnectionEvent> {
        self.event_tx.subscribe()
    }

    fn emit(&self, event: VoiceConnectionEvent) {
        let _ = self.event_tx.send(event);
    }

    pub fn set_session_id(&mut self, session_id: String) {
        self.session_id = session_id;
    }

    pub fn set_server(&mut self, token: String, endpoint: String) {
        self.token = token;
        self.endpoint = endpoint;
    }

    // ------------------------------------------------------------------
    // Connection lifecycle
    // ------------------------------------------------------------------

    pub async fn connect(&mut self) -> anyhow::Result<Arc<Mutex<VoiceSession>>> {
        let old = self.conn_state.lock().await.clone();
        self.set_conn_state(VoiceConnectionStatus::Connecting, None, None).await;

        let server_id = self.guild_id.clone();
        let session_id = self.session_id.clone();
        let token = self.token.clone();
        let user_id = self.user_id.clone();
        let channel_id = self.channel_id.clone();
        let preferred = self.preferred_mode;
        let ws_url = format!("wss://{}/?v=8", self.endpoint);

        let event_tx = self.event_tx.clone();
        let conn_state = self.conn_state.clone();
        let ssrc_map = self.ssrc_map.clone();
        let ping_atomic = self.ping.clone();
        let session_arc = self.session.clone();
        let dave_session = self.dave_session.clone();

        let (session_tx, session_rx) = oneshot::channel::<VoiceSessionInfo>();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            Self::run_voice_ws(
                server_id, session_id, user_id, channel_id, token, ws_url, session_tx,
                preferred, event_tx, conn_state, ssrc_map, ping_atomic, &mut shutdown_rx,
                session_arc, dave_session,
            )
            .await;
        });

        *self.ws_shutdown.lock().await = Some(shutdown_tx);
        *self.ws_handle.lock().await = Some(handle);

        let info = tokio::time::timeout(Duration::from_secs(15), session_rx)
            .await
            .map_err(|_| anyhow::anyhow!("Voice session setup timeout"))?
            .map_err(|_| anyhow::anyhow!("Voice session setup failed"))?;

        let user_id: u64 = self.user_id.parse().unwrap_or(0);
        let address = info.udp_socket.peer_addr()?;
        let session = VoiceSession {
            udp_socket: info.udp_socket.clone(),
            user_id,
            ssrc: info.ssrc,
            secret_key: info.secret_key,
            sequence: 0,
            timestamp: 0,
            address,
            encryption_mode: info.encryption_mode,
            dave_session: self.dave_session.clone(),
        };

        let return_arc = Arc::new(Mutex::new(session.clone()));
        *self.session.lock().await = Some(session);

        self.set_conn_state(VoiceConnectionStatus::Connected, None, None).await;
        let new_state = self.conn_state.lock().await.clone();
        self.emit(VoiceConnectionEvent::StateChange {
            old,
            new: new_state,
        });

        Ok(return_arc)
    }

    pub fn get_session(&self) -> Arc<Mutex<Option<VoiceSession>>> {
        self.session.clone()
    }

    pub async fn destroy(&mut self) {
        self.stop_internal().await;
        if let Some(tx) = self.ws_shutdown.lock().await.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.ws_handle.lock().await.take() {
            handle.abort();
        }
        if let Some(session) = self.session.lock().await.take() {
            drop(session);
        }
        let old = self.conn_state.lock().await.clone();
        self.set_conn_state(VoiceConnectionStatus::Destroyed, None, None).await;
        let new = self.conn_state.lock().await.clone();
        self.emit(VoiceConnectionEvent::StateChange { old, new });
    }

    // ------------------------------------------------------------------
    // Playback controls
    // ------------------------------------------------------------------

    pub async fn play(&self, mut resource: Box<dyn VoiceAudioResource + Send>) {
        self.stop_internal().await;

        let session_arc = self.session.clone();
        let event_tx = self.event_tx.clone();
        let player_state = self.player_state.clone();
        let stats = self.statistics.clone();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            let old = player_state.lock().await.clone();
            *player_state.lock().await = VoicePlayerState {
                status: VoicePlayerStatus::Playing,
                reason: Some("requested".into()),
            };
            let new = player_state.lock().await.clone();
            let _ = event_tx.send(VoiceConnectionEvent::PlayerStateChange { old, new });

            loop {
                tokio::select! {
                    frame = resource.next_opus_frame() => {
                        let frame = match frame {
                            Some(f) => f,
                            None => break,
                        };
                        let mut s = session_arc.lock().await;
                        if let Some(ref mut sess) = *s {
                            if sess.send_opus_frame(&frame).await.is_ok() {
                                let mut st = stats.lock().await;
                                st.packets_sent += 1;
                                st.packets_expected += 1;
                                st.bytes_sent += frame.len() as u64;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }

            let reason = if resource.position_ms() > 0 {
                "finished"
            } else {
                "stopped"
            };
            let old_p = player_state.lock().await.clone();
            *player_state.lock().await = VoicePlayerState {
                status: VoicePlayerStatus::Idle,
                reason: Some(reason.into()),
            };
            let new_p = player_state.lock().await.clone();
            let _ = event_tx.send(VoiceConnectionEvent::PlayerStateChange {
                old: old_p,
                new: new_p,
            });
        });

        *self.play_shutdown.lock().await = Some(shutdown_tx);
        *self.play_handle.lock().await = Some(handle);
    }

    pub async fn stop(&self, reason: Option<&str>) {
        self.stop_internal().await;
        let old = self.player_state.lock().await.clone();
        *self.player_state.lock().await = VoicePlayerState {
            status: VoicePlayerStatus::Idle,
            reason: reason.map(|s| s.to_string()),
        };
        let new = self.player_state.lock().await.clone();
        self.emit(VoiceConnectionEvent::PlayerStateChange { old, new });
    }

    pub async fn pause(&self) {
        let old = self.player_state.lock().await.clone();
        *self.player_state.lock().await = VoicePlayerState {
            status: VoicePlayerStatus::Paused,
            reason: None,
        };
        let new = self.player_state.lock().await.clone();
        self.emit(VoiceConnectionEvent::PlayerStateChange { old, new });
    }

    pub async fn unpause(&self) {
        let old = self.player_state.lock().await.clone();
        *self.player_state.lock().await = VoicePlayerState {
            status: VoicePlayerStatus::Playing,
            reason: Some("requested".into()),
        };
        let new = self.player_state.lock().await.clone();
        self.emit(VoiceConnectionEvent::PlayerStateChange { old, new });
    }

    async fn stop_internal(&self) {
        if let Some(tx) = self.play_shutdown.lock().await.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.play_handle.lock().await.take() {
            handle.abort();
        }
    }

    // ------------------------------------------------------------------
    // Speaking detection
    // ------------------------------------------------------------------

    pub async fn set_ssrc_mapping(&self, user_id: String, ssrc: u32) {
        let mut map = self.ssrc_map.lock().await;
        map.insert(ssrc, user_id);
    }

    pub async fn remove_ssrc_mapping(&self, ssrc: u32) {
        let mut map = self.ssrc_map.lock().await;
        map.remove(&ssrc);
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    async fn set_conn_state(
        &self,
        status: VoiceConnectionStatus,
        code: Option<u32>,
        reason: Option<String>,
    ) {
        let mut state = self.conn_state.lock().await;
        state.status = status;
        state.code = code;
        state.reason = reason;
    }

    async fn negotiate_mode(
        preferred: Option<EncryptionMode>,
        server_modes: &[String],
    ) -> EncryptionMode {
        if let Some(pref) = preferred {
            if server_modes.iter().any(|m| m == pref.as_str()) {
                return pref;
            }
        }
        for mode in EncryptionMode::all_supported() {
            if server_modes.iter().any(|m| m == mode.as_str()) {
                return mode;
            }
        }
        EncryptionMode::XSalsa20Poly1305
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_voice_ws(
        server_id: String,
        session_id: String,
        user_id: String,
        channel_id: String,
        token: String,
        ws_url: String,
        session_tx: oneshot::Sender<VoiceSessionInfo>,
        preferred_mode: Option<EncryptionMode>,
        event_tx: broadcast::Sender<VoiceConnectionEvent>,
        _conn_state: Arc<Mutex<VoiceConnectionState>>,
        ssrc_map: Arc<Mutex<HashMap<u32, String>>>,
        ping_atomic: Arc<AtomicI64>,
        shutdown_rx: &mut oneshot::Receiver<()>,
        session_arc: Arc<Mutex<Option<VoiceSession>>>,
        dave_session: DaveHandle,
    ) {
        info!(target: "Voice", "Starting voice WS url={ws_url} (server: {server_id})");
        let max_retries = 5;
        let mut retry_delay = Duration::from_millis(500);
        let mut first_attempt = true;
        let mut heartbeat_interval_ms: u64 = 41250;
        let mut session_tx = Some(session_tx);
        let mut selected_mode = EncryptionMode::XSalsa20Poly1305;

        for attempt in 0..max_retries {
            // Check for shutdown signal before attempting connection
            if shutdown_rx.try_recv().is_ok() {
                return;
            }

            let ws_stream = match connect_async(&ws_url).await {
                Ok((s, _)) => s,
                Err(e) => {
                    warn!(
                        target: "Voice",
                        "WS connect attempt {} failed: {e} (server: {})",
                        attempt + 1, server_id
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                    continue;
                }
            };

            let (mut write, mut read) = ws_stream.split();
            let mut session_info: Option<VoiceSessionInfo> = None;
            let mut handshake_completed = false;

            let payload = if first_attempt {
                json!({
                    "op": 0,
                    "d": {
                        "server_id": server_id,
                        "user_id": user_id,
                        "session_id": session_id,
                        "token": token,
                        "max_dave_protocol_version": DAVE_PROTOCOL_MAX,
                    }
                })
            } else {
                json!({
                    "op": 7,
                    "d": {
                        "server_id": server_id,
                        "session_id": session_id,
                        "token": token,
                    }
                })
            };
            first_attempt = false;

            info!(target: "Voice", "Sending OP {}: user_id={}, srv={}, sess={} (server: {})",
                payload["op"], user_id, server_id, session_id, server_id);

            if write
                .send(Message::Text(payload.to_string()))
                .await
                .is_err()
            {
                warn!(
                    target: "Voice",
                    "Failed to send voice op (server: {})",
                    server_id
                );
                first_attempt = true;
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                continue;
            }

            // Handshake loop
            let mut ssrc: u32 = 0;
            let user_id_u64: u64 = user_id.parse().unwrap_or(0);
            let channel_id_u64: u64 = channel_id.parse().unwrap_or(0);
            let mut dave_ver: u16 = 0;
            loop {
                let msg = tokio::select! {
                    msg = read.next() => msg,
                    _ = &mut *shutdown_rx => {
                        info!(target: "Voice", "Shutdown signal received (server: {})", server_id);
                        return;
                    }
                };
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let payload: serde_json::Value =
                            serde_json::from_str(&text).unwrap_or_default();
                        match payload["op"].as_u64().unwrap_or(0) {
                            2 => {
                                let ip = payload["d"]["ip"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                let port = payload["d"]["port"].as_u64().unwrap_or(0) as u16;
                                ssrc = payload["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                                info!(
                                    target: "Voice",
                                    "Ready: UDP {}:{}, SSRC {} (server: {})",
                                    ip, port, ssrc, server_id
                                );

                                let udp = match UdpSocket::bind("0.0.0.0:0").await {
                                    Ok(s) => s,
                                    Err(e) => {
                                        error!(target: "Voice", "UDP bind: {e}");
                                        break;
                                    }
                                };
                                let addr: SocketAddr = match format!("{}:{}", ip, port).parse() {
                                    Ok(a) => a,
                                    Err(e) => {
                                        error!(target: "Voice", "Invalid address: {e}");
                                        break;
                                    }
                                };
                                let _ = udp.connect(addr).await;

                                let mut discovery = vec![0u8; 74];
                                discovery[0..2].copy_from_slice(&1u16.to_be_bytes());
                                discovery[2..4].copy_from_slice(&70u16.to_be_bytes());
                                discovery[4..8].copy_from_slice(&ssrc.to_be_bytes());
                                let _ = udp.send(&discovery).await;

                                let mut recv_buf = vec![0u8; 74];
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(5),
                                    udp.recv(&mut recv_buf),
                                )
                                .await;

                                let ip_end = recv_buf[8..72]
                                    .iter()
                                    .position(|&b| b == 0)
                                    .unwrap_or(64);
                                let public_ip =
                                    String::from_utf8_lossy(&recv_buf[8..8 + ip_end]).to_string();
                                let public_port =
                                    u16::from_be_bytes([recv_buf[72], recv_buf[73]]);

                                let available_modes: Vec<String> = payload["d"]["modes"]
                                    .as_array()
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_else(|| {
                                        vec!["xsalsa20_poly1305".to_string()]
                                    });

                                selected_mode =
                                    Self::negotiate_mode(preferred_mode, &available_modes).await;
                                info!(
                                    target: "Voice",
                                    "Selected encryption mode: {} (server: {})",
                                    selected_mode.as_str(),
                                    server_id
                                );

                                let select = json!({
                                    "op": 1,
                                    "d": {
                                        "protocol": "udp",
                                        "data": {
                                            "address": public_ip,
                                            "port": public_port,
                                            "mode": selected_mode.as_str(),
                                        },
                                    },
                                });

                                if write
                                    .send(Message::Text(select.to_string()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                session_info = Some(VoiceSessionInfo {
                                    udp_socket: Arc::new(udp),
                                    ssrc,
                                    secret_key: [0u8; 32],
                                    encryption_mode: selected_mode,
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
                                    warn!(
                                        target: "Voice",
                                        "Invalid secret key length: {} (server: {})",
                                        key_hex.len(),
                                        server_id
                                    );
                                    break;
                                }

                                if let Some(ref mut si) = session_info {
                                    si.secret_key.copy_from_slice(&key_hex[..32]);
                                }

                                // DAVE session init
                                dave_ver = payload["d"]["dave_protocol_version"]
                                    .as_u64().unwrap_or(0) as u16;
                                if dave_ver > 0 {
                                    info!(target: "Voice", "DAVE protocol version {} requested (server: {})", dave_ver, server_id);
                                    let proto = NonZeroU16::new(dave_ver.min(DAVE_PROTOCOL_MAX))
                                        .unwrap_or(NonZeroU16::new(1).unwrap());
                                    let mut ds = dave_session.lock().await;
                                    match DaveSession::new(proto, user_id_u64, channel_id_u64, None) {
                                        Ok(mut s) => {
                                            match s.create_key_package() {
                                                Ok(kp) => {
                                                    *ds = Some(s);
                                                    let mut frame = vec![MLS_OPCODE_KEY_PACKAGE];
                                                    frame.extend_from_slice(&kp);
                                                    let _ = write.send(Message::Binary(frame.into())).await;
                                                    info!(target: "Voice", "DAVE session initialized and KEY_PACKAGE sent (server: {})", server_id);
                                                }
                                                Err(e) => {
                                                    warn!(target: "Voice", "DAVE failed to create key package: {e} (server: {})", server_id);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(target: "Voice", "Failed to init DAVE session: {e} (server: {})", server_id);
                                        }
                                    }
                                }

                                // Update shared session arc (for reconnections where session_tx is already consumed)
                                if let Some(ref si) = session_info {
                                    if let Ok(addr) = si.udp_socket.peer_addr() {
                                        let vs = VoiceSession {
                                            udp_socket: si.udp_socket.clone(),
                                            user_id: user_id_u64,
                                            ssrc: si.ssrc,
                                            secret_key: si.secret_key,
                                            sequence: 0,
                                            timestamp: 0,
                                            address: addr,
                                            encryption_mode: si.encryption_mode,
                                            dave_session: dave_session.clone(),
                                        };
                                        *session_arc.lock().await = Some(vs);
                                    }
                                }

                                if let Some(tx) = session_tx.take() {
                                    if let Some(si) = session_info.take() {
                                        let _ = tx.send(si);
                                        handshake_completed = true;
                                    }
                                } else {
                                    handshake_completed = true;
                                }

                                info!(
                                    target: "Voice",
                                    "Session description received (server: {})",
                                    server_id
                                );
                                break;
                            }
                            5 => {
                                // Speaking detection opcode
                                let speaking = payload["d"]["speaking"].as_u64().unwrap_or(0);
                                let ssrc = payload["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                                let map = ssrc_map.lock().await;
                                if let Some(uid) = map.get(&ssrc) {
                                    let event = if speaking != 0 {
                                        VoiceConnectionEvent::SpeakStart {
                                            user_id: uid.clone(),
                                            ssrc,
                                        }
                                    } else {
                                        VoiceConnectionEvent::SpeakEnd {
                                            user_id: uid.clone(),
                                            ssrc,
                                        }
                                    };
                                    let _ = event_tx.send(event);
                                }
                                drop(map);
                            }
                            6 => {
                                let nonce = payload["d"].as_u64().unwrap_or(0);
                                ping_atomic.store(nonce as i64, Ordering::Relaxed);
                            }
                            8 => {
                                heartbeat_interval_ms = payload["d"]["heartbeat_interval"]
                                    .as_u64()
                                    .unwrap_or(41250);
                                info!(
                                    target: "Voice",
                                    "Hello, heartbeat {}ms (server: {})",
                                    heartbeat_interval_ms,
                                    server_id
                                );
                            }
                            9 => {
                                info!(
                                    target: "Voice",
                                    "Resume failed, giving up (server: {})",
                                    server_id
                                );
                                return;
                            }
                            21 => {
                                // DAVE_PREPARE_TRANSITION
                                let mut ds = dave_session.lock().await;
                                if let Some(ref mut d) = *ds {
                                    let tid = payload["d"]["transition_id"].as_u64().unwrap_or(0) as u16;
                                    let ver = payload["d"]["protocol_version"].as_u64().unwrap_or(0) as u16;
                                    if ver == 0 {
                                        d.set_passthrough_mode(true, Some(10));
                                    }
                                    if tid != 0 {
                                        let ready = json!({"op": 23, "d": {"transition_id": tid}});
                                        let _ = write.send(Message::Text(ready.to_string())).await;
                                    }
                                }
                            }
                            22 => {
                                // DAVE_EXECUTE_TRANSITION — applied by server, nothing to do
                            }
                            24 => {
                                // DAVE_PREPARE_EPOCH
                                let epoch = payload["d"]["epoch"].as_u64().unwrap_or(0);
                                if epoch == 1 {
                                    warn!(target: "Voice", "DAVE epoch 1 reset, reinitializing session");
                                    let mut ds = dave_session.lock().await;
                                    *ds = None;
                                    let proto = NonZeroU16::new(dave_ver)
                                        .unwrap_or(NonZeroU16::new(1).unwrap());
                                    match DaveSession::new(proto, user_id_u64, channel_id_u64, None) {
                                        Ok(mut s) => {
                                            match s.create_key_package() {
                                                Ok(kp) => {
                                                    *ds = Some(s);
                                                    let mut frame = vec![MLS_OPCODE_KEY_PACKAGE];
                                                    frame.extend_from_slice(&kp);
                                                    let _ = write.send(Message::Binary(frame.into())).await;
                                                    info!(target: "Voice", "DAVE session reinitialized after epoch reset");
                                                }
                                                Err(e) => {
                                                    warn!(target: "Voice", "DAVE epoch reset key package failed: {e}");
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(target: "Voice", "DAVE epoch reset session init failed: {e}");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        Self::handle_dave_binary(&data, &dave_session, &mut write, user_id_u64, channel_id_u64, dave_ver).await;
                    }
                    Some(Ok(Message::Close(c))) => {
                        warn!(target: "Voice", "WS closed during handshake: {c:?} (server: {})", server_id);
                        first_attempt = true;
                        break;
                    }
                    _ => {
                        warn!(target: "Voice", "WS error during handshake (server: {})", server_id);
                        first_attempt = true;
                        break;
                    }
                }
            }

            if !handshake_completed {
                warn!(
                    target: "Voice",
                    "Voice WS handshake incomplete (server: {})",
                    server_id
                );
            } else {
                info!(target: "Voice", "Sending speaking notification (ssrc={}) (server: {})", ssrc, server_id);
                let speaking = json!({"op": 5, "d": {"speaking": 1, "delay": 0, "ssrc": ssrc, "user_id": user_id}});
                let _ = write.send(Message::Text(speaking.to_string())).await;
            }

            // Main loop: heartbeat + read
            let mut interval =
                tokio::time::interval(Duration::from_millis(heartbeat_interval_ms));
            interval.tick().await;
            let mut nonce: u64 = 0;
            let mut heartbeat_ok = true;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        nonce = nonce.wrapping_add(1);
                        let hb = json!({"op": 3, "d": nonce});
                        if write.send(Message::Text(hb.to_string())).await.is_err() {
                            heartbeat_ok = false;
                            break;
                        }
                    }
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let p: serde_json::Value =
                                    serde_json::from_str(&text).unwrap_or_default();
                                match p["op"].as_u64().unwrap_or(0) {
                                    5 => {
                                        let speaking = p["d"]["speaking"].as_u64().unwrap_or(0);
                                        let ssrc = p["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                                        let map = ssrc_map.lock().await;
                                        if let Some(uid) = map.get(&ssrc) {
                                            let evt = if speaking != 0 {
                                                VoiceConnectionEvent::SpeakStart {
                                                    user_id: uid.clone(),
                                                    ssrc,
                                                }
                                            } else {
                                                VoiceConnectionEvent::SpeakEnd {
                                                    user_id: uid.clone(),
                                                    ssrc,
                                                }
                                            };
                                            let _ = event_tx.send(evt);
                                        }
                                        drop(map);
                                    }
                                    6 => {
                                        let nonce = p["d"].as_u64().unwrap_or(0);
                                        ping_atomic.store(nonce as i64, Ordering::Relaxed);
                                    }
                                    21 => {
                                        let mut ds = dave_session.lock().await;
                                        if let Some(ref mut d) = *ds {
                                            let tid = p["d"]["transition_id"].as_u64().unwrap_or(0) as u16;
                                            let ver = p["d"]["protocol_version"].as_u64().unwrap_or(0) as u16;
                                            if ver == 0 {
                                                d.set_passthrough_mode(true, Some(10));
                                            }
                                            if tid != 0 {
                                                let ready = json!({"op": 23, "d": {"transition_id": tid}});
                                                let _ = write.send(Message::Text(ready.to_string())).await;
                                            }
                                        }
                                    }
                                    24 => {
                                        let epoch = p["d"]["epoch"].as_u64().unwrap_or(0);
                                        if epoch == 1 {
                                            warn!(target: "Voice", "DAVE epoch 1 reset, reinitializing session");
                                            let mut ds = dave_session.lock().await;
                                            *ds = None;
                                            let proto = NonZeroU16::new(dave_ver)
                                                .unwrap_or(NonZeroU16::new(1).unwrap());
                                            match DaveSession::new(proto, user_id_u64, channel_id_u64, None) {
                                                Ok(mut s) => {
                                                    match s.create_key_package() {
                                                        Ok(kp) => {
                                                            *ds = Some(s);
                                                            let mut frame = vec![MLS_OPCODE_KEY_PACKAGE];
                                                            frame.extend_from_slice(&kp);
                                                            let _ = write.send(Message::Binary(frame.into())).await;
                                                            info!(target: "Voice", "DAVE session reinitialized after epoch reset");
                                                        }
                                                        Err(e) => {
                                                            warn!(target: "Voice", "DAVE epoch reset key package failed: {e}");
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    warn!(target: "Voice", "DAVE epoch reset session init failed: {e}");
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Some(Ok(Message::Binary(data))) => {
                                Self::handle_dave_binary(&data, &dave_session, &mut write, user_id_u64, channel_id_u64, dave_ver).await;
                            }
                            Some(Ok(Message::Close(_))) | None => {
                                heartbeat_ok = false;
                                break;
                            }
                            _ => {}
                        }
                    }
                    _ = &mut *shutdown_rx => {
                        info!(target: "Voice", "Shutdown signal received, exiting WS loop (server: {})", server_id);
                        return;
                    }
                }
            }

            if !heartbeat_ok {
                info!(
                    target: "Voice",
                    "Voice WS disconnected, reconnecting (server: {})",
                    server_id
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                continue;
            }

            break;
        }

        info!(
            target: "Voice",
            "Voice WS task ended (server: {})",
            server_id
        );
    }

    // ------------------------------------------------------------------
    // DAVE binary opcode handlers
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn handle_dave_binary(
        data: &[u8],
        dave_session: &DaveHandle,
        write: &mut WsWrite,
        user_id: u64,
        channel_id: u64,
        protocol_version: u16,
    ) {
        if data.len() < 3 {
            if !data.is_empty() {
                warn!(target: "Voice", "DAVE binary message too short ({})", data.len());
            }
            return;
        }
        // Discord wraps MLS binary messages in a 2-byte header: [0x00] [subtype]
        // The actual MLS opcode (25-31) is at data[2]
        // For outgoing messages (bot→server), no header is needed.
        let opcode = data[2];
        let payload = &data[3..];

        match opcode {
            MLS_OPCODE_EXTERNAL_SENDER => {
                let mut ds = dave_session.lock().await;
                if let Some(ref mut s) = *ds {
                    if let Err(e) = s.set_external_sender(payload) {
                        warn!(target: "Voice", "DAVE external_sender: {e}");
                    }
                }
            }
            MLS_OPCODE_PROPOSALS => {
                if payload.is_empty() {
                    return;
                }
                let operation_type = payload[0];
                let proposals = &payload[1..];
                let mut ds = dave_session.lock().await;
                if let Some(ref mut s) = *ds {
                    let optype = if operation_type == 0 {
                        ProposalsOperationType::APPEND
                    } else {
                        ProposalsOperationType::REVOKE
                    };
                    match s.process_proposals(optype, proposals, None) {
                        Ok(Some(CommitWelcome { commit, welcome })) => {
                            // Wire format per daveprotocol.com dave_mls_commit_welcome (28):
                            // [opcode][MLSMessage commit][Welcome welcome?] — commit_message is
                            // NOT length-prefixed (it's a self-delimiting MLSMessage), so no
                            // varint goes here.
                            let mut frame = vec![MLS_OPCODE_COMMIT_WELCOME];
                            frame.extend_from_slice(&commit);
                            if let Some(ref w) = welcome {
                                frame.extend_from_slice(w);
                            }
                            let _ = write.send(Message::Binary(frame.into())).await;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!(target: "Voice", "DAVE process_proposals: {e}");
                        }
                    }
                }
            }
            MLS_OPCODE_ANNOUNCE_COMMIT => {
                if payload.len() < 2 {
                    return;
                }
                let transition_id = u16::from_be_bytes([payload[0], payload[1]]);
                // Wire format per daveprotocol.com dave_mls_announce_commit_transition (29):
                // [seq][opcode][transition_id][MLSMessage commit] — commit_message is NOT
                // length-prefixed, it's the raw remainder of the payload.
                let commit_data = &payload[2..];
                let mut ds = dave_session.lock().await;
                if let Some(ref mut s) = *ds {
                    if let Err(e) = s.process_commit(commit_data) {
                        warn!(target: "Voice", "DAVE process_commit: {e}");
                        // Session will be reset on INVALID_COMMIT from server
                    } else {
                        if s.is_ready() {
                            info!(target: "Voice", "DAVE session ready");
                        }
                        if transition_id != 0 {
                            let ready = json!({"op": 23, "d": {"transition_id": transition_id}});
                            let _ = write.send(Message::Text(ready.to_string())).await;
                        }
                    }
                }
            }
            MLS_OPCODE_WELCOME => {
                if payload.len() < 2 {
                    return;
                }
                let transition_id = u16::from_be_bytes([payload[0], payload[1]]);
                let welcome_data = &payload[2..];
                let mut ds = dave_session.lock().await;
                if let Some(ref mut s) = *ds {
                    if let Err(e) = s.process_welcome(welcome_data) {
                        warn!(target: "Voice", "DAVE process_welcome: {e}");
                    } else if transition_id != 0 {
                        let ready = json!({"op": 23, "d": {"transition_id": transition_id}});
                        let _ = write.send(Message::Text(ready.to_string())).await;
                    }
                    if s.is_ready() {
                        info!(target: "Voice", "DAVE session ready");
                    }
                }
            }
            MLS_OPCODE_INVALID_COMMIT => {
                warn!(target: "Voice", "DAVE invalid commit received, reinitializing");
                let mut ds = dave_session.lock().await;
                *ds = None;
                let proto = NonZeroU16::new(protocol_version)
                    .unwrap_or(NonZeroU16::new(1).unwrap());
                match DaveSession::new(proto, user_id, channel_id, None) {
                    Ok(mut s) => {
                    match s.create_key_package() {
                        Ok(kp) => {
                            *ds = Some(s);
                            let mut frame = vec![MLS_OPCODE_KEY_PACKAGE];
                            frame.extend_from_slice(&kp);
                            let _ = write.send(Message::Binary(frame.into())).await;
                            info!(target: "Voice", "DAVE session reinitialized after invalid commit");
                        }
                        Err(e) => {
                            warn!(target: "Voice", "DAVE reinit key package failed: {e}");
                        }
                    }
                    }
                    Err(e) => {
                        warn!(target: "Voice", "DAVE reinit failed: {e}");
                    }
                }
            }
            _ => {
                warn!(target: "Voice", "Unknown DAVE binary opcode: {opcode}");
            }
        }
    }
}