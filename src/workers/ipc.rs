/// IPC frame protocol matching NodeLink's binary frame format.
///
/// Frame format (6-byte header + variable id + variable payload):
///   Byte 0: idSize      (UInt8)   - length of the ID string
///   Byte 1: frameType   (UInt8)   - frame type
///   Bytes 2-5: payloadSize (UInt32BE) - length of payload
///   Following: id        (UTF-8 string, idSize bytes)
///   Following: payload   (binary, payloadSize bytes)

use bytes::{BufMut, Bytes, BytesMut};
use std::fmt;

/// Frame types used on command socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFrameType {
    Hello = 0,
    Command = 1,
    Result = 2,
    Error = 3,
    Ping = 5,
    Pong = 6,
    RotateSocket = 7,
}

impl CommandFrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Hello),
            1 => Some(Self::Command),
            2 => Some(Self::Result),
            3 => Some(Self::Error),
            5 => Some(Self::Ping),
            6 => Some(Self::Pong),
            7 => Some(Self::RotateSocket),
            _ => None,
        }
    }
}

/// Frame types used on event socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventFrameType {
    PlayerEvent = 3,
    WorkerStats = 4,
    StreamChunk = 5,
    StreamEnd = 6,
    StreamError = 7,
    VoiceRelayFrame = 8,
    LiveChatAction = 9,
}

/// Frame types used on source socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFrameType {
    Data = 0,
    End = 1,
    Error = 2,
    ChatAction = 3,
}

/// A decoded IPC frame.
#[derive(Debug, Clone)]
pub struct IpcFrame {
    pub frame_type: u8,
    pub id: String,
    pub payload: Bytes,
}

impl IpcFrame {
    /// Encode a frame into a BytesMut.
    pub fn encode(frame_type: u8, id: &str, payload: &[u8]) -> BytesMut {
        let id_bytes = id.as_bytes();
        let id_size = id_bytes.len().min(255) as u8;
        let payload_size = payload.len() as u32;

        let mut buf = BytesMut::with_capacity(6 + id_size as usize + payload_size as usize);
        buf.put_u8(id_size);
        buf.put_u8(frame_type);
        buf.put_u32(payload_size);
        buf.put_slice(&id_bytes[..id_size as usize]);
        buf.put_slice(payload);
        buf
    }

    /// Encode a JSON-serializable payload into a frame.
    pub fn encode_json<T: serde::Serialize>(frame_type: u8, id: &str, payload: &T) -> BytesMut {
        let json = serde_json::to_vec(payload).unwrap_or_default();
        Self::encode(frame_type, id, &json)
    }

    /// Try to parse a frame from a buffer. Returns the frame and remaining buffer.
    pub fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        if buf.len() < 6 {
            return None;
        }

        let id_size = buf[0] as usize;
        let frame_type = buf[1];
        let payload_size = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]) as usize;

        let total = 6 + id_size + payload_size;
        if buf.len() < total {
            return None;
        }

        let id = String::from_utf8_lossy(&buf[6..6 + id_size]).to_string();
        let payload = Bytes::copy_from_slice(&buf[6 + id_size..total]);

        Some((Self { frame_type, id, payload }, &buf[total..]))
    }
}

impl fmt::Display for IpcFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IpcFrame(type={}, id={}, payload_len={})", self.frame_type, self.id, self.payload.len())
    }
}

/// Socket paths for workers, matching NodeLink's createSocketPath.
pub fn make_worker_socket_path(name: &str) -> String {
    let rand_hex: String = (0..8)
        .map(|_| {
            let b: u8 = rand::random();
            format!("{:02x}", b)
        })
        .collect();
    #[cfg(target_os = "windows")]
    {
        format!("\\\\.\\pipe\\rustlink-{}-{}.sock", name, rand_hex)
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("/tmp/rustlink-{}-{}.sock", name, rand_hex)
    }
}

/// Worker type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerType {
    Playback = 0,
    Source = 1,
}

impl WorkerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkerType::Playback => "playback",
            WorkerType::Source => "source",
        }
    }
}

/// Command types for IPC (V8-serialized equivalent using JSON).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpcCommand {
    #[serde(rename = "type")]
    pub cmd_type: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IpcHello {
    pub pid: u32,
    pub worker_type: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkerStatsReport {
    pub players: u32,
    pub playing_players: u32,
    pub cpu_load: f64,
    pub frames_sent: u64,
    pub frames_nulled: u64,
    pub frames_deficit: u64,
    pub memory_used_bytes: u64,
    pub uptime_secs: u64,
    pub event_loop_lag_ms: f64,
}

/// Ping/pong health check message (sent via cluster IPC).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthCheckPing {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthCheckPong {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub timestamp: u64,
}

/// Socket rotation message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RotateSocketMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub event_socket_path: String,
    pub command_socket_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_encode_decode() {
        let payload = b"hello world";
        let encoded = IpcFrame::encode(1, "test-id", payload);
        let (frame, remaining) = IpcFrame::decode(&encoded).unwrap();
        assert_eq!(frame.frame_type, 1);
        assert_eq!(frame.id, "test-id");
        assert_eq!(&frame.payload[..], payload);
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_frame_encode_json() {
        let val = serde_json::json!({"speed": 1.5});
        let encoded = IpcFrame::encode_json(1, "req-1", &val);
        let (frame, _) = IpcFrame::decode(&encoded).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(decoded["speed"], 1.5);
    }

    #[test]
    fn test_partial_buffer() {
        let buf = vec![0u8; 4]; // Only 4 bytes, need 6 for header
        assert!(IpcFrame::decode(&buf).is_none());
    }

    #[test]
    fn test_long_id_truncation() {
        let long_id = "a".repeat(300);
        let encoded = IpcFrame::encode(0, &long_id, b"payload");
        let (frame, _) = IpcFrame::decode(&encoded).unwrap();
        assert_eq!(frame.id.len(), 255); // Truncated to 255
    }

    #[test]
    fn test_socket_path_format() {
        let path = make_worker_socket_path("test");
        assert!(path.starts_with(r"\\.\pipe\rustlink-test-"));
        assert!(path.ends_with(".sock"));
        assert_eq!(path.len(), 44); // \\.\pipe\rustlink-test-{16hex}.sock = 44
    }

    #[test]
    fn test_multiple_frames() {
        let f1 = IpcFrame::encode(1, "id1", b"payload1");
        let f2 = IpcFrame::encode(2, "id2", b"payload2");
        let mut combined = BytesMut::new();
        combined.extend_from_slice(&f1);
        combined.extend_from_slice(&f2);

        let (decoded1, rest) = IpcFrame::decode(&combined).unwrap();
        assert_eq!(decoded1.id, "id1");
        assert_eq!(decoded1.frame_type, 1);

        let (decoded2, rest) = IpcFrame::decode(rest).unwrap();
        assert_eq!(decoded2.id, "id2");
        assert_eq!(decoded2.frame_type, 2);
        assert!(rest.is_empty());
    }
}
