use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const MAGIC_READ_SIZE: u64 = 4096;

pub struct LocalProvider {
    base_path: PathBuf,
}

impl LocalProvider {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    fn resolve_path(&self, query: &str) -> anyhow::Result<PathBuf> {
        let input = Path::new(query);
        let path = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.base_path.join(input)
        };
        let path = path.canonicalize()?;

        if !input.is_absolute() {
            let base = self.base_path.canonicalize()?;
            if !path.starts_with(base) {
                anyhow::bail!("Path traversal is not allowed.");
            }
        }

        Ok(path)
    }

    fn read_magic_bytes(file_path: &Path, size: u64) -> Vec<u8> {
        use std::fs::File;
        use std::io::Read;
        let mut f = match File::open(file_path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        let mut buf = vec![0u8; size as usize];
        let n = f.read(&mut buf).unwrap_or(0);
        buf.truncate(n);
        buf
    }

    fn detect_type_by_magic(header: &[u8]) -> Option<&'static str> {
        if header.len() < 4 {
            return None;
        }

        // FLAC
        if &header[0..4] == b"fLaC" {
            return Some("audio/flac");
        }

        // Ogg
        if &header[0..4] == b"OggS" {
            if header.windows(8).any(|w| w == b"OpusHead") {
                return Some("audio/opus");
            }
            return Some("audio/ogg");
        }

        // WAV
        if header.len() >= 12
            && &header[0..4] == b"RIFF"
            && &header[8..12] == b"WAVE"
        {
            return Some("audio/wav");
        }

        // ID3 (MP3 with tags)
        if header.len() >= 3 && &header[0..3] == b"ID3" {
            return Some("audio/mpeg");
        }

        // MPEG frame sync
        if header[0] == 0xff && (header[1] & 0xe0) == 0xe0 {
            return Some("audio/mpeg");
        }

        // AAC (ADTS)
        if header[0] == 0xff && (header[1] & 0xf6) == 0xf0 {
            return Some("audio/aac");
        }

        // MP4/M4A (ftyp box)
        if header.len() >= 8 && &header[4..8] == b"ftyp" {
            return Some("m4a");
        }

        // WebM/Matroska (EBML header)
        if header.len() >= 4
            && header[0] == 0x1a
            && header[1] == 0x45
            && header[2] == 0xdf
            && header[3] == 0xa3
        {
            return Some("webm");
        }

        // FLV
        if header.len() >= 3 && &header[0..3] == b"FLV" {
            return Some("flv");
        }

        None
    }

    fn map_extension_to_type(extension: &str) -> &'static str {
        match extension.to_ascii_lowercase().as_str() {
            "mp3" => "audio/mpeg",
            "flac" => "audio/flac",
            "m4a" => "m4a",
            "mp4" => "mp4",
            "mov" => "mov",
            "aac" => "audio/aac",
            "wav" => "audio/wav",
            "ogg" => "audio/ogg",
            "opus" => "audio/opus",
            "webm" => "webm",
            "weba" => "weba",
            "flv" => "flv",
            _ => "arbitrary",
        }
    }

    fn detect_local_audio_type(file_path: &Path, extension: &str) -> &'static str {
        let header = Self::read_magic_bytes(file_path, MAGIC_READ_SIZE);
        Self::detect_type_by_magic(&header)
            .unwrap_or_else(|| Self::map_extension_to_type(extension))
    }

    fn parse_mp3_header(buf: &[u8]) -> Option<i64> {
        if buf.len() < 3 {
            return None;
        }
        let b1 = buf[0];
        let b2 = buf[1];
        let b3 = buf[2];
        if b1 != 0xff || (b2 & 0xe0) != 0xe0 {
            return None;
        }
        let version_bits = (b2 & 0x18) >> 3;
        let bitrate_index = (b3 & 0xf0) >> 4;
        if bitrate_index < 1 || bitrate_index > 14 {
            return None;
        }

        let version = match version_bits {
            3 => "1",
            2 => "2",
            0 => "2.5",
            _ => return None,
        };

        let bitrate = match version {
            "1" => match bitrate_index {
                1 => 32, 2 => 40, 3 => 48, 4 => 56, 5 => 64, 6 => 80, 7 => 96,
                8 => 112, 9 => 128, 10 => 160, 11 => 192, 12 => 224, 13 => 256,
                14 => 320, _ => return None,
            },
            "2" | "2.5" => match bitrate_index {
                1 => 8, 2 => 16, 3 => 24, 4 => 32, 5 => 40, 6 => 48, 7 => 56,
                8 => 64, 9 => 80, 10 => 96, 11 => 112, 12 => 128, 13 => 144,
                14 => 160, _ => return None,
            },
            _ => return None,
        };

        Some(bitrate)
    }

    fn detect_id3v2_size(file_path: &Path) -> u64 {
        let header = Self::read_magic_bytes(file_path, 10);
        if header.len() < 10 {
            return 0;
        }
        if header[0] != 0x49 || header[1] != 0x44 || header[2] != 0x33 {
            return 0;
        }
        let size = ((header[6] as u64 & 0x7f) << 21)
            | ((header[7] as u64 & 0x7f) << 14)
            | ((header[8] as u64 & 0x7f) << 7)
            | (header[9] as u64 & 0x7f);
        size + 10
    }

    fn read_file_info(file_path: &Path) -> serde_json::Value {
        let extension = file_path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();

        let stream_type = Self::detect_local_audio_type(file_path, &extension);

        let mut info = json!({
            "fileType": extension,
            "streamType": stream_type,
            "bitrateKbps": "unknown",
            "durationMs": -1,
        });

        if extension == "mp3" {
            let skip = Self::detect_id3v2_size(file_path);
            let header_start = skip.min(Self::read_magic_bytes(file_path, MAGIC_READ_SIZE).len() as u64);
            let buf = if header_start > 0 {
                let all = Self::read_magic_bytes(file_path, MAGIC_READ_SIZE + header_start);
                if all.len() > header_start as usize {
                    all[header_start as usize..].to_vec()
                } else {
                    Vec::new()
                }
            } else {
                Self::read_magic_bytes(file_path, MAGIC_READ_SIZE)
            };

            if let Some(bitrate_kbps) = Self::parse_mp3_header(&buf) {
                if let Ok(metadata) = fs::metadata(file_path) {
                    let file_size = metadata.len();
                    let bits_per_second = (bitrate_kbps as u64) * 1000;
                    let duration_ms = if bits_per_second > 0 {
                        ((file_size * 8) / bits_per_second) * 1000 / 8
                    } else {
                        0
                    };
                    info["bitrateKbps"] = json!(bitrate_kbps);
                    info["durationMs"] = json!(duration_ms as i64);
                }
            }
        }

        info
    }
}

#[async_trait]
impl SourceProvider for LocalProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn search(
        &self,
        query: &str,
        _search_type: Option<&str>,
    ) -> anyhow::Result<SourceResult> {
        self.resolve(query, None).await
    }

    async fn resolve(&self, path: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let Ok(path) = self.resolve_path(path) else {
            return Ok(SourceResult::empty());
        };

        if !path.is_file() {
            return Ok(SourceResult::empty());
        }

        let title = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("Unknown")
            .to_owned();

        let uri = path.to_string_lossy().to_string();

        let plugin_info = Self::read_file_info(&path);
        let stream_type = plugin_info["streamType"].as_str().unwrap_or("arbitrary");
        let duration_ms = plugin_info["durationMs"].as_i64().unwrap_or(-1);
        let is_seekable = duration_ms > 0;

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: uri.clone(),
                is_seekable,
                author: "unknown".into(),
                length: duration_ms,
                is_stream: false,
                position: 0,
                title,
                uri: Some(uri),
                artwork_url: None,
                isrc: None,
                source_name: "local".into(),
                chapters: None,
            },
            plugin_info: json!({ "streamType": stream_type }),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));

        Ok(SourceResult::Track(track))
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let stream_type = track
            .uri
            .as_ref()
            .map(|u| Path::new(u))
            .and_then(|p| {
                let ext = p
                    .extension()
                    .and_then(|v| v.to_str())
                    .unwrap_or("");
                Some(Self::detect_local_audio_type(p, ext))
            })
            .unwrap_or("arbitrary");

        Ok(TrackUrlResult {
            url: track.uri.clone(),
            protocol: Some("local".into()),
            format: json!(stream_type),
            new_track: None,
            additional_data: json!({}),
            exception: None,
        })
    }
}
